//! JSON parsing utilities with safety limits.
//!
//! serde_json has a default recursion depth limit of 128, which prevents
//! stack-overflow DoS from deeply nested JSON. This module adds an input
//! size limit to prevent memory-exhaustion DoS from excessively large inputs.

const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;

/// Parse JSON with a size limit to prevent memory exhaustion DoS.
pub fn parse<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, crate::ToolError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(crate::ToolError::invalid_input(format!(
            "input exceeds maximum length of {} bytes",
            MAX_INPUT_BYTES
        )));
    }
    serde_json::from_str(input).map_err(|e| crate::ToolError::serialization(e.to_string()))
}
