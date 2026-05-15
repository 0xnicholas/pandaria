#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file (if present)
    // Errors are silently ignored so the file is optional.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter("pandaria_tui=info")
        .init();
    tui::run().await
}
