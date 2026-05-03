pub fn auth_header(token: &str) -> String {
    format!("Bearer {}", token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_header_format() {
        assert_eq!(auth_header("abc123"), "Bearer abc123");
    }

    #[test]
    fn test_auth_header_empty_token() {
        assert_eq!(auth_header(""), "Bearer ");
    }
}
