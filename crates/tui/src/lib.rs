pub mod app;
pub mod autocomplete;
pub mod bash_mode;
pub mod client;
pub mod clipboard;
pub mod command;
pub mod component;
pub mod config;
pub mod dev_token;
pub mod input_queue;
pub mod keybindings;
pub mod markdown;
pub mod overlays;
pub mod paste;
pub mod state;
pub mod ui;
pub mod widgets;

use app::App;
use clap::Parser;
use config::{CliArgs, Config};
use crossterm::ExecutableCommand;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::time::Duration;
use widgets::spinner::SpinnerWidget;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = CliArgs::parse();
    let mut config = Config::load(cli)?;
    let rest = client::rest::RestClient::new(&config.server);

    // Resolve auth token: use explicit config, or auto-generate a dev token
    // for localhost development servers.
    let token = match config.auth.token.clone() {
        Some(t) => t,
        None => {
            let url = &config.server.url;
            let dev_tokens = dev_token::dev_tokens(url);
            if dev_tokens.is_empty() {
                return Err(
                    "No auth token. Set PANDARIA_TOKEN env var, --token flag, or config file."
                        .into(),
                );
            }

            let mut last_err = None;
            for candidate in &dev_tokens {
                match rest.list_sessions(candidate).await {
                    Ok(_) => {
                        tracing::info!("Auto-generated dev token for local server at {}", url);
                        config.auth.token = Some(candidate.clone());
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }

            match config.auth.token.clone() {
                Some(t) => t,
                None => {
                    tracing::error!(
                        "Failed to auto-authenticate with local server at {}. \
                         Is the server running? Last error: {:?}",
                        url,
                        last_err
                    );
                    return Err(format!(
                        "Could not connect to local server at {}. \
                         Ensure pandaria-server is running, or provide an explicit token.",
                        url
                    )
                    .into());
                }
            }
        }
    };

    let sessions = rest.list_sessions(&token).await?;
    let session_info = if let Some(first) = sessions.into_iter().next() {
        first
    } else {
        rest.create_session(None, &token).await?
    };

    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    stdout.execute(EnableBracketedPaste)?;
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
                    Some(Ok(Event::Paste(data))) => {
                        app.handle_paste(data);
                    }
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
            task_action = async {
                match app.task_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(action) = task_action { app.handle_task_action(action); }
            }
            _ = spinner_interval.tick() => {
                if app.state == app::AppState::Busy { app.spinner.tick(); }
            }
        }
    }

    terminal.backend_mut().execute(DisableBracketedPaste)?;
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    Ok(())
}
