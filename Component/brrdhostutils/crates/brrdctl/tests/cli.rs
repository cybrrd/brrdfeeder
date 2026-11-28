// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use std::process::{Command, Output};
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brrdctl"))
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn top_level_help() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("status"));
    assert!(text.contains("Show system information"));
    assert!(!text.contains("doctor"));
    assert!(!text.contains("--json"));
}
#[test]
fn command_help() {
    let output = run(&["status", "--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--system"));
    assert!(text.contains("CPU, memory and disk"));
}
#[test]
fn version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("brrdctl {}\n", env!("CARGO_PKG_VERSION"))
    );
}
#[test]
fn unknown_command() {
    let output = run(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("frobnicate"));
    assert!(text.contains("unrecognized"));
    assert!(text.contains("brrdctl --help"));
}
#[test]
fn missing_selector() {
    let output = run(&["status"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("report must be chosen"));
    assert!(text.contains("--system"));
    assert!(text.contains("--help"));
    assert!(text.contains("Usage: brrdctl status [OPTIONS]"));
}
#[test]
fn no_arguments() {
    let output = run(&[]);
    let help = run(&["--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, help.stdout);
}
#[test]
fn one_shot_report() {
    let start = std::time::Instant::now();
    let output = run(&["status", "--system"]);
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    for label in [
        "CPU cores: ",
        "CPU usage: ",
        "Load (1, 5, 15 min): ",
        "Memory: ",
        "Swap: ",
        "Disks (used / total):",
        "  /: ",
    ] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    let cpu = text
        .lines()
        .find_map(|line| line.strip_prefix("CPU usage: "))
        .unwrap();
    assert!(cpu.strip_suffix('%').unwrap().parse::<u8>().unwrap() <= 100);
    assert_eq!(text.matches("CPU cores:").count(), 1);
    assert!(!text.contains("measuring"));
}
#[test]
fn plain_output() {
    let output = run(&["status", "--system"]);
    assert!(output.status.success());
    assert!(!output.stdout.contains(&27));
    assert!(output.stdout.iter().all(|b| *b >= 32 || *b == b'\n'));
}
#[test]
fn ordinary_user() {
    assert_ne!(std::env::var("USER").unwrap(), "root");
    let output = Command::new(env!("CARGO_BIN_EXE_brrdctl"))
        .env_clear()
        .args(["status", "--system"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("unavailable"));
    assert!(text.contains("Memory:"));
    assert!(text.contains("  /: "));
}

#[path = "../../brrd-config/tests/support/mod.rs"]
mod support;
#[test]
fn same_report() {
    let data = support::snapshot();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    assert_eq!(
        brrdctl::run(
            ["brrdctl", "status", "--system"],
            || data.clone(),
            &mut output,
            &mut errors
        )
        .unwrap(),
        0
    );
    assert!(errors.is_empty());
    let mut app = brrd_config::app::App::new(support::Fixed(data));
    app.intent(brrd_config::app::Intent::Open);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let body: Vec<String> = (4..23)
        .map(|y| {
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .take_while(|line| !line.is_empty())
        .collect();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text, body.join("\n") + "\n");
    assert!(text.contains("3.7 GiB"));
}
#[test]
fn unreadable_cli() {
    let mut data = support::snapshot();
    data.load = brrd_core::Reading::Unavailable;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    assert_eq!(
        brrdctl::run(
            ["brrdctl", "status", "--system"],
            || data,
            &mut output,
            &mut errors
        )
        .unwrap(),
        0
    );
    assert!(errors.is_empty());
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Load (1, 5, 15 min): unavailable"));
    assert!(text.contains("Memory: 1.0 GiB / 3.7 GiB (27%)"));
}
#[test]
fn help_and_errors_never_measure() {
    for args in [
        vec!["brrdctl"],
        vec!["brrdctl", "--help"],
        vec!["brrdctl", "status"],
        vec!["brrdctl", "bad"],
    ] {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let code = brrdctl::run(
            args,
            || panic!("unexpected measurement"),
            &mut output,
            &mut errors,
        )
        .unwrap();
        assert!(code == 0 || code == 2);
    }
}
