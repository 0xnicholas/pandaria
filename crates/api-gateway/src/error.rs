use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;



/// Gateway 错误类型。
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error(transparent)]
    Tenant(#[from] tenant::TenantError),

    #[error("invalid session id")]
    InvalidSessionId,

    #[error("session not found")]
    SessionNotFound,

    #[error("rate limit exceeded")]
    RateLimited,

    #[error("unauthorized")]
    Unauthorized,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::Tenant(tenant_err) => match tenant_err {
                tenant::TenantError::TenantNotFound(_) => {
                    (StatusCode::NOT_FOUND, "not_found", tenant_err.to_string())
                }
                tenant::TenantError::SessionNotFound(_) => {
                    (StatusCode::NOT_FOUND, "not_found", "session not found".into())
                }
                tenant::TenantError::TenantAlreadyExists(_) => {
                    (StatusCode::CONFLICT, "conflict", tenant_err.to_string())
                }
                tenant::TenantError::SessionLimitExceeded { .. } => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "limit_exceeded",
                    tenant_err.to_string(),
                ),
                tenant::TenantError::TokenBudgetExceeded { .. } => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "limit_exceeded",
                    tenant_err.to_string(),
                ),
                tenant::TenantError::ToolCallRateLimitExceeded { .. } => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    tenant_err.to_string(),
                ),
                tenant::TenantError::Internal(msg) => {
                    tracing::error!(error = %msg, "tenant internal error");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal",
                        "internal error".into(),
                    )
                }
            },
            Self::InvalidSessionId => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                self.to_string(),
            ),
            Self::SessionNotFound => (StatusCode::NOT_FOUND, "not_found", "session not found".into()),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "rate limit exceeded".into(),
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "invalid or missing token".into(),
            ),
        };

        let body = Json(json!({
            "error": {
                "code": code,
                "message": message,
            }
        }));

        let mut response = (status, body).into_response();

        if status == StatusCode::TOO_MANY_REQUESTS && matches!(self, Self::RateLimited) {
            response
                .headers_mut()
                .insert("Retry-After", "1".parse().expect("literal '1' is valid header value"));
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unauthorized_response() {
        let response = GatewayError::Unauthorized.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_rate_limited_response() {
        let response = GatewayError::RateLimited.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get("Retry-After").unwrap(),
            "1"
        );
    }

    #[test]
    fn test_tenant_session_not_found() {
        let err = GatewayError::Tenant(tenant::TenantError::SessionNotFound("s1".into()));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_tenant_internal_hides_details() {
        let err = GatewayError::Tenant(tenant::TenantError::Internal("secret".into()));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
