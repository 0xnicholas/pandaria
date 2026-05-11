use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use dashmap::DashMap;

use agent_core::context::{ProviderRequestCtx, TurnEndCtx};
use agent_core::mutations::ProviderRequestMutation;

use crate::host::extension::Extension;

/// TokenBudget extension — 每会话 turn 消耗配额观测（v0）。
///
/// v0 实现按 turn 次数计量：
/// - `on_turn_end`：累加会话 turn 计数
/// - `on_before_provider_request`：检查是否超过配额，超限时记录 warning 日志
///
/// # 架构限制说明
/// `on_before_provider_request` 是 chain hook（非阻断型），因此 v0 无法直接阻断超限请求。
/// 阻断能力将在 v1 中由 tenant 模块在更高层统一实现，或待 agent-core 提供阻断型 provider hook。
///
/// # 使用示例
/// ```
/// use extensions::builtins::token_budget::TokenBudgetExtension;
///
/// let ext = TokenBudgetExtension::new(100); // 每会话最多 100 turns
/// ```
pub struct TokenBudgetExtension {
    max_turns_per_session: usize,
    /// session_id -> turn count
    session_turn_counts: DashMap<String, AtomicUsize>,
}

impl TokenBudgetExtension {
    /// 创建 TokenBudget。
    ///
    /// # 参数
    /// - `max_turns_per_session`: 每会话允许的最多 turn 数
    pub fn new(max_turns_per_session: usize) -> Self {
        Self {
            max_turns_per_session,
            session_turn_counts: DashMap::new(),
        }
    }

    /// 获取当前会话的累计 turn 数（用于测试和观测）。
    pub fn current_turn_count(&self, session_id: &str) -> usize {
        self.session_turn_counts
            .get(session_id)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0)
    }
}

#[async_trait]
impl Extension for TokenBudgetExtension {
    fn name(&self) -> &str {
        "token-budget"
    }

    async fn on_before_provider_request(
        &self,
        ctx: &ProviderRequestCtx,
    ) -> ProviderRequestMutation {
        let count = self.current_turn_count(&ctx.session_id);

        if count >= self.max_turns_per_session {
            tracing::warn!(
                target: "pandaria.token_budget",
                tenant_id = %ctx.tenant_id,
                session_id = %ctx.session_id,
                turn_count = count,
                max_turns = self.max_turns_per_session,
                action = "budget_exceeded"
            );
        }

        ProviderRequestMutation::default()
    }

    async fn on_turn_end(&self, ctx: &TurnEndCtx) {
        let entry = self
            .session_turn_counts
            .entry(ctx.session_id.clone())
            .or_insert_with(|| AtomicUsize::new(0));

        let new_count = entry.fetch_add(1, Ordering::SeqCst) + 1;

        tracing::info!(
            target: "pandaria.token_budget",
            tenant_id = %ctx.tenant_id,
            session_id = %ctx.session_id,
            turn_index = ctx.turn_index,
            accumulated_turns = new_count,
            max_turns = self.max_turns_per_session,
            action = "turn_recorded"
        );

        if new_count >= self.max_turns_per_session {
            tracing::warn!(
                target: "pandaria.token_budget",
                tenant_id = %ctx.tenant_id,
                session_id = %ctx.session_id,
                accumulated_turns = new_count,
                max_turns = self.max_turns_per_session,
                action = "budget_threshold_reached"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_turn_counting() {
        let ext = TokenBudgetExtension::new(5);

        let ctx = TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 0,
            messages: vec![],
        };

        ext.on_turn_end(&ctx).await;
        ext.on_turn_end(&ctx).await;
        ext.on_turn_end(&ctx).await;

        assert_eq!(ext.current_turn_count("s1"), 3);
    }

    #[tokio::test]
    async fn test_multi_session_isolation() {
        let ext = TokenBudgetExtension::new(10);

        ext.on_turn_end(&TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 0,
            messages: vec![],
        }).await;

        ext.on_turn_end(&TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s2".to_string(),
            turn_index: 0,
            messages: vec![],
        }).await;
        ext.on_turn_end(&TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s2".to_string(),
            turn_index: 1,
            messages: vec![],
        }).await;

        assert_eq!(ext.current_turn_count("s1"), 1);
        assert_eq!(ext.current_turn_count("s2"), 2);
    }

    #[tokio::test]
    async fn test_budget_exceeded_check() {
        let ext = TokenBudgetExtension::new(2);

        // 记录 2 个 turns，达到阈值
        ext.on_turn_end(&TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 0,
            messages: vec![],
        }).await;
        ext.on_turn_end(&TurnEndCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            turn_index: 1,
            messages: vec![],
        }).await;

        assert_eq!(ext.current_turn_count("s1"), 2);

        // 下次 provider request 时检查，应触发 budget_exceeded 日志（不阻断）
        let provider_ctx = ProviderRequestCtx {
            tenant_id: "t1".to_string(),
            session_id: "s1".to_string(),
            model: "test".to_string(),
            system_prompt: None,
            messages: vec![],
            turn_index: 2,
            tools: None,
            options: agent_core::provider_opts::ProviderStreamOptions::default(),
        };

        let mutation = ext.on_before_provider_request(&provider_ctx).await;
        // 由于 on_before_provider_request 是 chain hook，只能返回 mutation，不能 Block
        assert!(mutation.messages.is_none());
    }
}
