// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use crossterm::event::{self, Event};
use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub enum Message {
    Input(Event),
    Tick,
    Error(io::Error),
}

/// A single input worker also schedules ticks, even during continuous input.
pub fn start() -> Receiver<Message> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let interval = Duration::from_secs(1);
        let mut deadline = Instant::now() + interval;
        loop {
            if Instant::now() >= deadline {
                if sender.send(Message::Tick).is_err() {
                    break;
                }
                deadline += interval;
            }
            match event::poll(deadline.saturating_duration_since(Instant::now())) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if sender.send(Message::Input(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Message::Error(error));
                        break;
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    let _ = sender.send(Message::Error(error));
                    break;
                }
            }
        }
    });
    receiver
}
