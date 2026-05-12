use regex::Regex;
use std::sync::Arc;
use std::sync::LazyLock;

use async_trait::async_trait;

use agent_core::context::{ToolCallCtx, ToolResultCtx};
use agent_core::mutations::{HookDecision, ToolCallMutation, ToolResultMutation};

use crate::host::extension::Extension;

/// PII 类型枚举
#[derive(Debug, Clone)]
pub enum PIIType {
    Email,
    Phone,
    CreditCard,
}

static EMAIL_RE: LazyLock<Arc<Regex>> = LazyLock::new(|| {
    Arc::new(
        Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
            .expect("hardcoded PII email regex is valid"),
    )
});

static PHONE_RE: LazyLock<Arc<Regex>> = LazyLock::new(|| {
    Arc::new(
        Regex::new(r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b")
            .expect("hardcoded PII phone regex is valid"),
    )
});

static CREDIT_CARD_RE: LazyLock<Arc<Regex>> = LazyLock::new(|| {
    Arc::new(
        Regex::new(r"\b(?:\d[ -]*?){13,16}\b")
            .expect("hardcoded PII credit card regex is valid"),
    )
});

/// 过滤规则
#[derive(Debug, Clone)]
pub enum FilterRule {
    Keyword(String),
    Regex(String),
    PII(PIIType),
}

/// 过滤动作
#[derive(Debug, Clone, Copy)]
pub enum FilterAction {
    /// 阻断请求（仅 `on_tool_call` 支持）
    Block,
    /// 脱敏替换
    Redact,
    /// 仅记录日志
    Log,
}

/// ContentFilter extension — 输入/输出内容过滤与脱敏。
///
/// 支持关键词匹配、正则表达式、PII 识别三种规则类型，可配置 Block / Redact / Log 动作。
///
/// ## 使用示例
/// ```
/// use extensions::builtins::content_filter::{ContentFilterExtension, FilterRule, FilterAction, PIIType};
///
/// let ext = ContentFilterExtension::new(vec![
///     (FilterRule::PII(PIIType::Email), FilterAction::Redact),
///     (FilterRule::Keyword("secret".to_string()), FilterAction::Block),
/// ]);
/// ```
pub struct ContentFilterExtension {
    rules: Vec<(FilterRule, FilterAction)>,
    /// 预编译的正则缓存（索引与 rules 对应，非 Regex 规则位置为 None）
    regex_cache: Vec<Option<Arc<Regex>>>,
}

impl ContentFilterExtension {
    /// 创建 ContentFilter。
    ///
    /// # Panics
    /// 如果 `FilterRule::Regex` 包含非法正则表达式，会在构造时 panic。
    pub fn new(rules: Vec<(FilterRule, FilterAction)>) -> Self {
        let mut regex_cache = Vec::with_capacity(rules.len());
        for (rule, _) in &rules {
            let re = match rule {
                FilterRule::Regex(pattern) => {
                    Some(Arc::new(Regex::new(pattern).expect("invalid regex in content filter")))
                }
                FilterRule::PII(pii_type) => Some(Arc::clone(match pii_type {
                    PIIType::Email => &EMAIL_RE,
                    PIIType::Phone => &PHONE_RE,
                    PIIType::CreditCard => &CREDIT_CARD_RE,
                })),
                _ => None,
            };
            regex_cache.push(re);
        }
        Self { rules, regex_cache }
    }

    /// 递归地对 JSON value 中的所有字符串进行脱敏。
    fn redact_json_value(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                *s = self.redact_text(s);
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    self.redact_json_value(v);
                }
            }
            serde_json::Value::Object(obj) => {
                for (_, v) in obj {
                    self.redact_json_value(v);
                }
            }
            _ => {}
        }
    }

    /// 对纯文本进行脱敏。
    fn redact_text(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (idx, (rule, action)) in self.rules.iter().enumerate() {
            if !matches!(action, FilterAction::Redact) {
                continue;
            }
            match rule {
                FilterRule::Keyword(kw) => {
                    result = result.replace(kw, "[REDACTED]");
                }
                FilterRule::Regex(_) => {
                    if let Some(re) = self.regex_cache.get(idx).and_then(|o| o.as_ref()) {
                        result = re.replace_all(&result, "[REDACTED]").to_string();
                    }
                }
                FilterRule::PII(pii_type) => {
                    let replacement = match pii_type {
                        PIIType::Email => "[REDACTED_EMAIL]",
                        PIIType::Phone => "[REDACTED_PHONE]",
                        PIIType::CreditCard => "[REDACTED_CREDIT_CARD]",
                    };
                    if let Some(re) = self.regex_cache.get(idx).and_then(|o| o.as_ref()) {
                        result = re.replace_all(&result, replacement).to_string();
                    }
                }
            }
        }
        result
    }

    /// 检查文本，返回匹配到的规则索引和匹配文本。
    fn check_text(&self, text: &str) -> Vec<(usize, String)> {
        let mut matches = Vec::new();
        for (idx, (rule, _)) in self.rules.iter().enumerate() {
            let matched = match rule {
                FilterRule::Keyword(kw) => {
                    if text.contains(kw) {
                        Some(kw.clone())
                    } else {
                        None
                    }
                }
                FilterRule::Regex(_) => self
                    .regex_cache
                    .get(idx)
                    .and_then(|o| o.as_ref())
                    .and_then(|re| re.find(text))
                    .map(|m| m.as_str().to_string()),
                FilterRule::PII(_) => {
                    self.regex_cache
                        .get(idx)
                        .and_then(|o| o.as_ref())
                        .and_then(|re| re.find(text))
                        .map(|m| m.as_str().to_string())
                }
            };
            if let Some(m) = matched {
                matches.push((idx, m));
            }
        }
        matches
    }
}

#[async_trait]
impl Extension for ContentFilterExtension {
    fn name(&self) -> &str {
        "content-filter"
    }

    async fn on_tool_call(
        &self,
        ctx: &ToolCallCtx,
    ) -> (HookDecision, ToolCallMutation) {
        // 将 input 转为字符串检查
        let text = match &ctx.input {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };

        let matches = self.check_text(&text);

        for (idx, matched_text) in matches {
            let (_, action) = &self.rules[idx];
            match action {
                FilterAction::Block => {
                    tracing::warn!(
                        target: "pandaria.content_filter",
                        tenant_id = %ctx.tenant_id,
                        session_id = %ctx.session_id,
                        tool_name = %ctx.tool_name,
                        matched = %matched_text,
                        rule_idx = idx,
                        action = "block"
                    );
                    return (
                        HookDecision::Block {
                            reason: format!("content filter blocked: matched rule {}", idx),
                        },
                        ToolCallMutation::default(),
                    );
                }
                FilterAction::Log => {
                    tracing::info!(
                        target: "pandaria.content_filter",
                        tenant_id = %ctx.tenant_id,
                        session_id = %ctx.session_id,
                        tool_name = %ctx.tool_name,
                        matched = %matched_text,
                        rule_idx = idx,
                        action = "log"
                    );
                }
                FilterAction::Redact => {
                    // on_tool_call 中 redact：修改 input
                    let mut mutated_input = ctx.input.clone();
                    self.redact_json_value(&mut mutated_input);
                    tracing::info!(
                        target: "pandaria.content_filter",
                        tenant_id = %ctx.tenant_id,
                        session_id = %ctx.session_id,
                        tool_name = %ctx.tool_name,
                        action = "redact_input"
                    );
                    return (
                        HookDecision::Continue,
                        ToolCallMutation {
                            input: Some(mutated_input),
                        },
                    );
                }
            }
        }

        (HookDecision::Continue, ToolCallMutation::default())
    }

    async fn on_tool_result(
        &self,
        ctx: &ToolResultCtx,
    ) -> ToolResultMutation {
        let mut mutated = false;
        let mut new_content = Vec::with_capacity(ctx.content.len());

        for content in &ctx.content {
            if let llm_client::Content::Text { text, text_signature } = content {
                let redacted = self.redact_text(text);
                if redacted != *text {
                    mutated = true;
                }
                new_content.push(llm_client::Content::Text {
                    text: redacted,
                    text_signature: text_signature.clone(),
                });
            } else {
                new_content.push(content.clone());
            }
        }

        let new_details = ctx.details.as_ref().map(|d| {
            let text = d.to_string();
            let redacted = self.redact_text(&text);
            if redacted != text {
                mutated = true;
            }
            serde_json::Value::String(redacted)
        });

        if mutated {
            tracing::info!(
                target: "pandaria.content_filter",
                tenant_id = %ctx.tenant_id,
                session_id = %ctx.session_id,
                tool_name = %ctx.tool_name,
                action = "redact_tool_result"
            );
            ToolResultMutation {
                content: Some(new_content),
                details: new_details,
                is_error: None,
                terminate: None,
            }
        } else {
            ToolResultMutation::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_email() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::PII(PIIType::Email), FilterAction::Redact),
        ]);

        let result = ext.redact_text("Contact me at alice@example.com or bob@test.org");
        assert_eq!(result, "Contact me at [REDACTED_EMAIL] or [REDACTED_EMAIL]");
    }

    #[test]
    fn test_redact_phone() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::PII(PIIType::Phone), FilterAction::Redact),
        ]);

        let result = ext.redact_text("Call 555-123-4567 or 800.987.6543");
        assert!(result.contains("[REDACTED_PHONE]"));
    }

    #[test]
    fn test_redact_keyword() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::Keyword("secret".to_string()), FilterAction::Redact),
        ]);

        let result = ext.redact_text("The secret password is secret123");
        assert_eq!(result, "The [REDACTED] password is [REDACTED]123");
    }

    #[tokio::test]
    async fn test_block_keyword() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::Keyword("password".to_string()), FilterAction::Block),
        ]);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "echo".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({"text": "my password is 123"}),
        };

        let (decision, _) = ext.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Block { .. }));
    }

    #[tokio::test]
    async fn test_log_only() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::Keyword("test".to_string()), FilterAction::Log),
        ]);

        let ctx = ToolCallCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "echo".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({"text": "this is a test"}),
        };

        let (decision, _) = ext.on_tool_call(&ctx).await;
        assert!(matches!(decision, HookDecision::Continue));
    }

    #[tokio::test]
    async fn test_redact_tool_result() {
        let ext = ContentFilterExtension::new(vec![
            (FilterRule::PII(PIIType::Email), FilterAction::Redact),
        ]);

        let ctx = ToolResultCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            tool_name: "echo".to_string(),
            tool_call_id: "c1".to_string(),
            input: serde_json::json!({}),
            content: vec![llm_client::Content::Text {
                text: "Email: user@example.com".to_string(),
                text_signature: None,
            }],
            details: None,
            is_error: false,
        };

        let mutation = ext.on_tool_result(&ctx).await;
        assert!(mutation.content.is_some());
        if let Some(content) = mutation.content {
            if let llm_client::Content::Text { text, .. } = &content[0] {
                assert_eq!(text, "Email: [REDACTED_EMAIL]");
            }
        }
    }
}
