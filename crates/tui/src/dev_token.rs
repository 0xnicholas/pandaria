/// Development token helpers for local testing with Aspectus.
///
/// With the migration to Aspectus as the single identity source, the TUI no longer
/// generates HMAC tokens. Instead, users provide an Aspectus-issued API key
/// (format: `pk_live_*`) through the `--token` flag or `PANDARIA_TOKEN` env var.
///
/// For local development, start Aspectus alongside pandaria:
///   cd ../Aspectus && docker compose up -d && sqlx migrate run
///   cargo run -p aspectus-server
///
/// Then get an API key from the Aspectus management API and pass it to the TUI:
///   pandaria-tui --url http://localhost:8080 --token pk_live_your_key
///
/// The legacy HMAC token generation has been removed (v0.2.0).

/// Return true if the URL points to a local development server.
fn is_local_dev(url: &str) -> bool {
    url.contains("localhost") || url.contains("127.0.0.1")
}

/// Return a helpful message for local development setup.
pub fn dev_setup_hint(url: &str) -> Option<String> {
    if !is_local_dev(url) {
        return None;
    }
    Some(
        "Local dev detected. Start Aspectus for authentication:\n\
         cd ../Aspectus && docker compose up -d\n\
         Then set PANDARIA_TOKEN to your API key.".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_dev() {
        assert!(is_local_dev("http://localhost:8080"));
        assert!(is_local_dev("http://127.0.0.1:8080"));
        assert!(!is_local_dev("https://api.example.com"));
    }

    #[test]
    fn test_dev_setup_hint_for_localhost() {
        let hint = dev_setup_hint("http://localhost:8080");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("Aspectus"));
    }

    #[test]
    fn test_dev_setup_hint_for_remote() {
        let hint = dev_setup_hint("https://api.example.com");
        assert!(hint.is_none());
    }
}
