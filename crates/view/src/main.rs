//! `skym-view`: a terminal view of skym-server, for people. Read-only.

mod api;
mod app;
mod apps;
mod connect;
mod names;
mod problems;
mod ui;

use api::Client;
use app::{App, Key, Msg, update};
use clap::Parser;
use jiff::Timestamp;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(name = "skym-view", version, about = "A terminal view of skym-server (read-only)")]
struct Args {
    /// Server URL, instead of SKYM_URL
    #[arg(long)]
    server: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    let connection = connect::resolve(
        args.server,
        |k| std::env::var(k).ok(),
        &connect::parse_env_file(&env_file()),
    );
    let client = match connection
        .map_err(anyhow::Error::msg)
        .and_then(|c| Client::new(&c.server, c.token))
    {
        Ok(client) => Arc::new(client),
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::from(2);
        }
    };
    let theme = ui::Theme { color: std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) };
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, client, theme).await;
    ratatui::restore();
    match result {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(why)) => {
            eprintln!("error: {why}");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// `~/.config/skym/env`; absent is fine, unreadable is said.
fn env_file() -> String {
    let Some(home) = std::env::var_os("HOME") else { return String::new() };
    let path = std::path::Path::new(&home).join(".config/skym/env");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            eprintln!("warning: cannot read {}: {e}", path.display());
            String::new()
        }
    }
}

/// Draw, wait for a key, a second or an answer, update, send the requests it asks for.
/// Returns why the view had to stop, if it did not just quit.
async fn run(
    terminal: &mut DefaultTerminal,
    client: Arc<Client>,
    theme: ui::Theme,
) -> anyhow::Result<Option<String>> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let keys = tx.clone();
    // Reading the terminal blocks: it gets a thread of its own.
    std::thread::spawn(move || {
        loop {
            let msg = match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => key(k).map(Msg::Key),
                Ok(Event::Resize(..)) => Some(Msg::Tick),
                Ok(_) => None,
                Err(e) => Some(Msg::Failed(format!("cannot read the terminal: {e}"))),
            };
            let failed = matches!(msg, Some(Msg::Failed(_)));
            if msg.is_some_and(|m| keys.send(m).is_err()) || failed {
                break; // the view has quit, or the terminal is gone
            }
        }
    });
    let mut app = App::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        terminal.draw(|f| ui::draw(f, &app, client.server(), Timestamp::now(), theme))?;
        let msg = tokio::select! {
            Some(msg) = rx.recv() => msg,
            _ = tick.tick() => Msg::Tick,
        };
        for request in update(&mut app, msg, Timestamp::now()) {
            let (client, tx) = (client.clone(), tx.clone());
            tokio::spawn(async move {
                let result = client.fetch(&request).await.map(Box::new);
                // Sending fails only once the view has quit; the answer is no longer wanted.
                drop(tx.send(Msg::Fetched(request, result)));
            });
        }
        if app.quit {
            return Ok(app.fatal.take());
        }
    }
}

fn key(k: KeyEvent) -> Option<Key> {
    Some(match k.code {
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Key::Quit,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}
