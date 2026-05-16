use tracing::Span;

/// 在 auth span 下创建带 session_id 的子 span。
pub fn session_span(session_id: &str) -> Span {
    tracing::info_span!(parent: Span::current(), "handler", session_id = %session_id)
}
