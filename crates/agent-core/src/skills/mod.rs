pub mod injector;
pub mod loader;
pub mod scanner;
pub mod types;

pub use injector::*;
pub use loader::*;
pub use types::*;

/// Parse a `/skill:name` invocation string.
///
/// Returns the skill name (without the `/skill:` prefix) if the text starts
/// with that prefix, otherwise `None`.
pub fn parse_skill_invocation(text: &str) -> Option<&str> {
    text.strip_prefix("/skill:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_invocation_valid() {
        assert_eq!(
            parse_skill_invocation("/skill:code-review"),
            Some("code-review")
        );
    }

    #[test]
    fn test_parse_skill_invocation_no_prefix() {
        assert_eq!(parse_skill_invocation("hello world"), None);
    }

    #[test]
    fn test_parse_skill_invocation_empty_name() {
        assert_eq!(parse_skill_invocation("/skill:"), Some(""));
    }

    #[test]
    fn test_parse_skill_invocation_with_extra_text() {
        // Current spec: the whole text must be `/skill:name`
        // parse_skill_invocation only strips prefix, caller decides if extra
        // text is acceptable. For now we just return the remainder.
        assert_eq!(
            parse_skill_invocation("/skill:code-review extra"),
            Some("code-review extra")
        );
    }
}
