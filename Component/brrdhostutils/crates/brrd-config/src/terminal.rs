// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        is_raw_mode_enabled,
    },
};
use std::io::{self, stdout};

/// Restores terminal modes on normal return, error, and panic unwinding.
pub struct TerminalGuard {
    restore_raw: bool,
}
impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        let guard = Self {
            restore_raw: !is_raw_mode_enabled()?,
        };
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), Show, LeaveAlternateScreen);
        if self.restore_raw {
            let _ = disable_raw_mode();
        }
    }
}
