// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use brrd_config::{
    app::{App, MenuItem, Section},
    pages,
};
use brrd_core::{Snapshot, SnapshotSource};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
mod support;
use support::Fixed;
fn app() -> App {
    App::new(Fixed(support::snapshot()))
}
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn screen(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
static THREE: [Section; 5] = [
    Section {
        name: "Status",
        short: "status",
        items: &[
            MenuItem {
                label: "First",
                open: pages::system_info,
            },
            MenuItem {
                label: "System Info",
                open: pages::system_info,
            },
            MenuItem {
                label: "Last",
                open: pages::system_info,
            },
        ],
    },
    Section {
        name: "Configuration",
        short: "config",
        items: &[],
    },
    Section {
        name: "Events",
        short: "events",
        items: &[],
    },
    Section {
        name: "Advanced Tools",
        short: "tools",
        items: &[],
    },
    Section {
        name: "About",
        short: "about",
        items: &[],
    },
];
fn three() -> App {
    App::with_menu(Fixed(support::snapshot()), &THREE)
}
#[test]
fn frame_at_start() {
    let text = screen(&app(), 80, 24);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 24);
    assert!(lines.iter().all(|l| l.len() == 80));
    for name in [
        "brrd-config",
        "[1 status]",
        "2 config",
        "3 events",
        "4 tools",
        "5 about",
    ] {
        assert!(lines[0].contains(name));
    }
    assert!(lines[1].trim().is_empty());
    assert_eq!(lines[2].trim(), "Status");
    assert!(lines[23].contains("exit"));
    insta::assert_snapshot!(text);
}
#[test]
fn status_section() {
    let mut app = app();
    key(&mut app, KeyCode::Char('1'));
    insta::assert_snapshot!(screen(&app, 80, 24));
}
#[test]
fn empty_section() {
    let mut app = app();
    key(&mut app, KeyCode::Char('2'));
    let text = screen(&app, 80, 24);
    assert!(!text.contains("> System"));
    assert!(!text.contains("open"));
    insta::assert_snapshot!(text);
}
#[test]
fn move_down() {
    let mut app = three();
    key(&mut app, KeyCode::Down);
    let text = screen(&app, 80, 24);
    assert!(text.contains("> System Info"));
    assert_eq!(text.lines().filter(|l| l.starts_with('>')).count(), 1);
    insta::assert_snapshot!(text);
}
#[test]
fn stop_at_end() {
    let mut app = three();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    let last = screen(&app, 80, 24);
    key(&mut app, KeyCode::Down);
    assert_eq!(screen(&app, 80, 24), last);
    insta::assert_snapshot!(last);
    key(&mut app, KeyCode::Up);
    assert!(screen(&app, 80, 24).contains("> System Info"));
    key(&mut app, KeyCode::Up);
    let first = screen(&app, 80, 24);
    key(&mut app, KeyCode::Up);
    assert_eq!(screen(&app, 80, 24), first);
}
#[test]
fn open_with_enter() {
    let mut app = three();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    assert!(text.contains("Status > System Info"));
    insta::assert_snapshot!(text);
}
#[test]
fn open_with_right() {
    let mut app = three();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    let text = screen(&app, 80, 24);
    assert!(text.contains("Status > System Info"));
    insta::assert_snapshot!(text);
}
#[test]
fn back() {
    for back in [KeyCode::Esc, KeyCode::Left] {
        let mut app = three();
        key(&mut app, KeyCode::Down);
        let before = screen(&app, 80, 24);
        key(&mut app, KeyCode::Enter);
        key(&mut app, back);
        assert_eq!(screen(&app, 80, 24), before);
    }
    let mut app = app();
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    insta::assert_snapshot!(screen(&app, 80, 24));
}
#[test]
fn back_at_top() {
    let mut app = three();
    let before = screen(&app, 80, 24);
    for back in [KeyCode::Left, KeyCode::Esc] {
        key(&mut app, back);
        assert_eq!(screen(&app, 80, 24), before);
    }
    insta::assert_snapshot!(before);
}
#[test]
fn jump_from_page() {
    let mut app = app();
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('5'));
    let text = screen(&app, 80, 24);
    assert_eq!(text.lines().nth(2).unwrap().trim(), "About");
    assert!(text.lines().next().unwrap().contains("[5 about]"));
    insta::assert_snapshot!(text);
}
#[test]
fn reserved_number() {
    let mut app = app();
    key(&mut app, KeyCode::Enter);
    let before = screen(&app, 80, 24);
    for c in ['6', '7', '8', '9'] {
        key(&mut app, KeyCode::Char(c));
        assert_eq!(screen(&app, 80, 24), before);
    }
    insta::assert_snapshot!(before);
}
#[test]
fn list_hints() {
    let text = screen(&three(), 80, 24);
    let hints = text.lines().last().unwrap();
    for name in ["Up/Down", "Enter/Right", "1-5", "0/q/Ctrl-C"] {
        assert!(hints.contains(name));
    }
    assert!(!hints.contains("back"));
    insta::assert_snapshot!(text);
}
#[test]
fn page_hints() {
    let mut app = app();
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    let hints = text.lines().last().unwrap();
    assert!(hints.contains("Esc/Left back"));
    assert!(!hints.contains("open"));
    insta::assert_snapshot!(text);
}
#[test]
fn started_too_small() {
    let mut app = app();
    let text = screen(&app, 60, 20);
    assert!(text.contains("80x24"));
    assert!(text.contains("60x20"));
    key(&mut app, KeyCode::Char('0'));
    assert!(app.quit);
    insta::assert_snapshot!(text);
}
#[test]
fn enlarged() {
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    let app = app();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal.backend_mut().resize(80, 24);
    terminal.autoresize().unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let text = (0..24)
        .map(|y| {
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text, screen(&app, 80, 24));
    insta::assert_snapshot!(text);
}
#[test]
fn monochrome() {
    let text = screen(&app(), 80, 24);
    assert_eq!(text.matches('[').count(), 1);
    assert_eq!(text.matches("> System Info").count(), 1);
    insta::assert_snapshot!(text);
}

#[test]
fn system_info_page() {
    let mut app = app();
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    assert_eq!(text.lines().nth(2).unwrap().trim(), "Status > System Info");
    for line in support::snapshot().report_lines() {
        assert!(
            text.lines().any(|actual| actual.trim_end() == line),
            "missing: {line}"
        );
    }
    assert!(text.lines().nth(1).unwrap().trim().is_empty());
    insta::assert_snapshot!(text);
}
#[test]
fn first_reading() {
    let mut data = support::snapshot();
    data.cpu = brrd_core::Reading::Measuring;
    let mut app = App::new(Fixed(data));
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    assert!(text.contains("CPU usage: measuring"));
    assert!(!text.contains("37%"));
    insta::assert_snapshot!(text);
}
#[test]
fn no_swap() {
    let mut data = support::snapshot();
    data.swap = brrd_core::Reading::Value(brrd_core::Usage { used: 0, total: 0 });
    let mut app = App::new(Fixed(data));
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    assert!(text.contains("Swap: none"));
    insta::assert_snapshot!(text);
}
#[test]
fn one_unavailable() {
    let mut data = support::snapshot();
    data.load = brrd_core::Reading::Unavailable;
    let mut app = App::new(Fixed(data));
    key(&mut app, KeyCode::Enter);
    let text = screen(&app, 80, 24);
    assert!(text.contains("Load (1, 5, 15 min): unavailable"));
    assert!(text.contains("Memory: 1.0 GiB / 3.7 GiB (27%)"));
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .contains("System information could not be read")
    );
    insta::assert_snapshot!(text);
}
#[test]
fn all_unavailable() {
    let mut app = App::new(Fixed(Snapshot::default()));
    key(&mut app, KeyCode::Enter);
    app.tick();
    let text = screen(&app, 80, 24);
    assert_eq!(text.matches("unavailable").count(), 7);
    assert!(!app.quit);
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .contains("System information could not be read")
    );
    insta::assert_snapshot!(text);
}
#[test]
fn tick_refresh() {
    struct Sequence(std::collections::VecDeque<Snapshot>);
    impl SnapshotSource for Sequence {
        fn snapshot(&mut self) -> Snapshot {
            self.0.pop_front().expect("unexpected reading")
        }
    }
    let mut next = support::snapshot();
    next.memory = brrd_core::Reading::Value(brrd_core::Usage {
        used: 2 << 30,
        total: 3_884_000 * 1024,
    });
    next.load = brrd_core::Reading::Unavailable;
    next.read_at =
        brrd_core::Reading::Value(std::time::UNIX_EPOCH + std::time::Duration::from_secs(45_297));
    let mut app = App::new(Sequence(
        [support::snapshot(), next, Snapshot::default()].into(),
    ));
    app.tick(); // No source read while the page is closed.
    key(&mut app, KeyCode::Enter);
    assert!(screen(&app, 80, 24).contains("1.0 GiB / 3.7 GiB"));
    app.tick();
    let text = screen(&app, 80, 24);
    assert!(text.contains("2.0 GiB / 3.7 GiB"));
    assert!(text.contains("12:34:57 UTC"));
    assert!(text.contains("Load (1, 5, 15 min): unavailable"));
    assert!(!text.contains("0.25, 0.50, 0.75"));
    insta::assert_snapshot!(text);
    app.tick();
    let failed = screen(&app, 80, 24);
    assert!(!failed.contains("2.0 GiB"));
    assert!(!failed.contains("12:34:57"));
    assert_eq!(failed.matches("unavailable").count(), 7);
}

#[test]
fn problem_clears() {
    struct Recovery(std::collections::VecDeque<Snapshot>);
    impl SnapshotSource for Recovery {
        fn snapshot(&mut self) -> Snapshot {
            self.0.pop_front().expect("unexpected reading")
        }
    }
    let mut failed = support::snapshot();
    failed.load = brrd_core::Reading::Unavailable;
    let mut app = App::new(Recovery([failed, support::snapshot()].into()));
    key(&mut app, KeyCode::Enter);
    let before = screen(&app, 80, 24);
    assert!(
        before
            .lines()
            .nth(1)
            .unwrap()
            .contains("System information could not be read")
    );
    insta::assert_snapshot!("problem_clears_before", before);
    app.tick();
    let after = screen(&app, 80, 24);
    assert!(after.lines().nth(1).unwrap().trim().is_empty());
    assert!(after.contains("Load (1, 5, 15 min): 0.25, 0.50, 0.75"));
    insta::assert_snapshot!("problem_clears_after", after);
}
