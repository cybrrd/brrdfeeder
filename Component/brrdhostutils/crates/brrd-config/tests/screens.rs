use brrd_config::{
    app::{App, MenuItem, Section},
    pages,
};
use brrd_core::{Snapshot, SnapshotSource};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
struct Fixed;
impl SnapshotSource for Fixed {
    fn snapshot(&mut self) -> Snapshot {
        Snapshot::default()
    }
}
fn app() -> App {
    App::new(Fixed)
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
    App::with_menu(Fixed, &THREE)
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
