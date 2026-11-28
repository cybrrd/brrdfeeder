// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use std::process::Command;
#[test]
fn pty_exit_keys() {
    let result = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/terminal_check.py"
        ))
        .arg(env!("CARGO_BIN_EXE_brrd-config"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
#[ignore = "deliberately panics in a subprocess with a terminal"]
fn panic_probe() {
    let _guard = brrd_config::terminal::TerminalGuard::enter().unwrap();
    panic!("provoked terminal failure");
}
#[test]
fn pty_panic_restores() {
    let result = Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/panic_check.py"))
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
