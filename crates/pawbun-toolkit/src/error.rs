use thiserror::Error;

/// 工具执行过程中可能发生的错误。
///
/// 所有错误变体均实现 [`Clone`]，便于在需要复制结果的场景（如缓存、重试）中使用。
/// 注意：包含 `source` 的变体在 `clone` 时会丢弃 `source` 链，仅保留消息文本。
///
/// # Example
/// ```
/// use pawbun_toolkit::ToolError;
///
/// let err = ToolError::NotFound("unknown_tool".into());
/// assert_eq!(err.to_string(), "tool not found: unknown_tool");
/// ```
#[derive(Error, Debug)]
pub enum ToolError {
    /// 输入参数无效或格式错误。
    #[error("invalid input: {message}")]
    InvalidInput {
        /// Error message describing the invalid input.
        message: String,
        /// Underlying cause of the error, if any.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// 工具执行过程中发生不可恢复的错误。
    #[error("execution failed: {message}")]
    ExecutionFailed {
        /// Error message describing the execution failure.
        message: String,
        /// Underlying cause of the error, if any.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// 请求的工具未在注册表中找到。
    #[error("tool not found: {0}")]
    NotFound(String),

    /// 工具执行超过指定超时时间。
    #[error("timeout after {0}ms")]
    Timeout(u64),

    /// 输入输出序列化或反序列化失败。
    #[error("serialization error: {message}")]
    Serialization {
        /// Error message describing the serialization failure.
        message: String,
        /// Underlying cause of the error, if any.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// IO error (file system, network, etc.).
    #[error("IO error: {message} (kind: {kind:?})")]
    Io {
        /// Error message describing the IO failure.
        message: String,
        /// IO error kind classification.
        kind: std::io::ErrorKind,
    },
}

impl Clone for ToolError {
    fn clone(&self) -> Self {
        match self {
            Self::InvalidInput { message, .. } => Self::InvalidInput {
                message: message.clone(),
                source: None,
            },
            Self::ExecutionFailed { message, .. } => Self::ExecutionFailed {
                message: message.clone(),
                source: None,
            },
            Self::NotFound(s) => Self::NotFound(s.clone()),
            Self::Timeout(ms) => Self::Timeout(*ms),
            Self::Serialization { message, .. } => Self::Serialization {
                message: message.clone(),
                source: None,
            },
            Self::Io { message, kind } => Self::Io {
                message: message.clone(),
                kind: *kind,
            },
        }
    }
}

impl ToolError {
    /// 创建 `InvalidInput` 错误（向后兼容的快捷构造函数）。
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: msg.into(),
            source: None,
        }
    }

    /// 创建 `ExecutionFailed` 错误（向后兼容的快捷构造函数）。
    pub fn execution_failed(msg: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: msg.into(),
            source: None,
        }
    }

    /// 创建 `Serialization` 错误（向后兼容的快捷构造函数）。
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization {
            message: msg.into(),
            source: None,
        }
    }

    /// 为当前错误附加根因（source）。
    ///
    /// 仅对 `InvalidInput`、`ExecutionFailed`、`Serialization` 有效；
    /// 其他变体返回自身不变。
    pub fn with_source(
        self,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        match self {
            Self::InvalidInput { message, .. } => Self::InvalidInput {
                message,
                source: Some(Box::new(source)),
            },
            Self::ExecutionFailed { message, .. } => Self::ExecutionFailed {
                message,
                source: Some(Box::new(source)),
            },
            Self::Serialization { message, .. } => Self::Serialization {
                message,
                source: Some(Box::new(source)),
            },
            other => other,
        }
    }
}
