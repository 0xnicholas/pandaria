use tui::app::App;
use tui::config::{CliArgs, Config};
use tui::client::rest::RestClient;
use tui::widgets::spinner::SpinnerWidget;
use clap::Parser;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("pandaria_tui=info")
        .init();
    let cli = CliArgs::parse();
    let config = Config::load(cli)?;
    let token = config
        .auth
        .token
        .clone()
        .ok_or("No auth token. Set PANDARIA_TOKEN env var, --token flag, or config file.")?;
    let rest = RestClient::new(&config.server);
    let sessions = rest
        .list_sessions(&token)
        .await
        .map_err(|e| format!("Failed to list sessions: {}", e))?;
    let session_info = if sessions.is_empty() {
        rest.create_session(None, &token)
            .await
            .map_err(|e| format!("Failed to create session: {}", e))?
    } else {
        sessions.into_iter().next().unwrap()
    };

    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, session_info.id.clone(), session_info);
    let mut spinner_interval =
        tokio::time::interval(Duration::from_millis(SpinnerWidget::interval_ms()));
    let mut reader = EventStream::new();

    loop {
        if !app.running {
            break;
        }
        terminal.draw(|f| app.render_ui(f))?;
        tokio::select! {
            event = reader.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => app.handle_key_event(key),
                    Some(Ok(Event::Resize(_, _))) => {},
                    Some(Err(e)) => tracing::error!("crossterm error: {}", e),
                    None => break,
                    _ => {}
                }
            }
            server_event = async {
                match app.server_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(event) = server_event { app.handle_server_event(event); }
            }
            _ = spinner_interval.tick() => {
                if app.state == tui::app::AppState::Busy { app.spinner.tick(); }
            }
        }
    }

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    Ok(())
}
