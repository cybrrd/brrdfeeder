// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use brrd_config::{
    app::App,
    events::{self, Message},
    terminal::TerminalGuard,
};
use brrd_core::SystemReader;
use crossterm::event::Event;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

fn run() -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new(SystemReader::default());
    let events = events::start();
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        match events.recv().map_err(io::Error::other)? {
            Message::Input(Event::Key(key)) => app.key(key),
            Message::Tick => app.tick(),
            Message::Error(error) => return Err(error),
            _ => {}
        }
        if app.quit {
            return Ok(());
        }
    }
}
fn main() -> io::Result<()> {
    run()
}
