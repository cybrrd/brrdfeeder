// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
use crate::pages::{self, Page};
use brrd_core::SnapshotSource;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{Frame, layout::Rect, widgets::Paragraph};
use std::{cell::RefCell, rc::Rc};

pub type Source = Rc<RefCell<Box<dyn SnapshotSource>>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    MoveUp,
    MoveDown,
    Open,
    Back,
    JumpTo(usize),
    Quit,
}
#[derive(Clone)]
pub struct Binding {
    pub keys: Vec<(KeyCode, KeyModifiers)>,
    pub intent: Intent,
    pub hint: &'static str,
}
impl Binding {
    fn new(codes: &[KeyCode], intent: Intent, hint: &'static str) -> Self {
        Self {
            keys: codes
                .iter()
                .map(|code| (*code, KeyModifiers::NONE))
                .collect(),
            intent,
            hint,
        }
    }
}
pub struct MenuItem {
    pub label: &'static str,
    pub open: fn(Source) -> Box<dyn Page>,
}
pub struct Section {
    pub name: &'static str,
    pub short: &'static str,
    pub items: &'static [MenuItem],
}
pub static MENU: [Section; 5] = [
    Section {
        name: "Status",
        short: "status",
        items: &[MenuItem {
            label: "System Info",
            open: pages::system_info,
        }],
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
pub struct App {
    menu: &'static [Section],
    section: usize,
    selected: usize,
    page: Option<Box<dyn Page>>,
    source: Source,
    pub quit: bool,
    message: String,
}
impl App {
    pub fn new(source: impl SnapshotSource + 'static) -> Self {
        Self::with_menu(source, &MENU)
    }
    pub fn with_menu(source: impl SnapshotSource + 'static, menu: &'static [Section]) -> Self {
        Self {
            menu,
            section: 0,
            selected: 0,
            page: None,
            source: Rc::new(RefCell::new(Box::new(source))),
            quit: false,
            message: String::new(),
        }
    }
    pub fn bindings(&self) -> Vec<Binding> {
        let mut bindings = if let Some(page) = &self.page {
            let mut bindings = page.bindings();
            bindings.push(Binding::new(
                &[KeyCode::Esc, KeyCode::Left],
                Intent::Back,
                "Esc/Left back",
            ));
            bindings
        } else if !self.menu[self.section].items.is_empty() {
            vec![
                Binding::new(&[KeyCode::Up], Intent::MoveUp, "Up/Down move"),
                Binding::new(&[KeyCode::Down], Intent::MoveDown, ""),
                Binding::new(
                    &[KeyCode::Enter, KeyCode::Right],
                    Intent::Open,
                    "Enter/Right open",
                ),
            ]
        } else {
            vec![]
        };
        for index in 0..self.menu.len() {
            bindings.push(Binding::new(
                &[KeyCode::Char(char::from(b'1' + index as u8))],
                Intent::JumpTo(index),
                if index == 0 { "1-5 section" } else { "" },
            ));
        }
        let mut quit = Binding::new(
            &[KeyCode::Char('0'), KeyCode::Char('q')],
            Intent::Quit,
            "0/q/Ctrl-C exit",
        );
        quit.keys.push((KeyCode::Char('c'), KeyModifiers::CONTROL));
        bindings.push(quit);
        bindings
    }
    pub fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if let Some(binding) = self
            .bindings()
            .iter()
            .find(|b| b.keys.contains(&(key.code, key.modifiers)))
        {
            self.intent(binding.intent);
        }
    }
    pub fn intent(&mut self, intent: Intent) {
        match intent {
            Intent::Quit => self.quit = true,
            Intent::JumpTo(section) if section < self.menu.len() => {
                self.section = section;
                self.selected = 0;
                self.page = None;
            }
            Intent::Back => self.page = None,
            Intent::MoveUp if self.page.is_none() => {
                self.selected = self.selected.saturating_sub(1)
            }
            Intent::MoveDown if self.page.is_none() => {
                self.selected =
                    (self.selected + 1).min(self.menu[self.section].items.len().saturating_sub(1))
            }
            Intent::Open if self.page.is_none() => {
                if let Some(item) = self.menu[self.section].items.get(self.selected) {
                    self.page = Some((item.open)(self.source.clone()));
                    self.refresh_message();
                }
            }
            other => {
                if let Some(page) = &mut self.page {
                    page.intent(other);
                }
            }
        }
    }
    pub fn tick(&mut self) {
        if let Some(page) = &mut self.page {
            page.tick();
            self.refresh_message();
        }
    }
    fn refresh_message(&mut self) {
        self.message = self
            .page
            .as_ref()
            .and_then(|page| page.message())
            .unwrap_or_default()
            .to_owned();
    }
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 80 || area.height < 24 {
            frame.render_widget(
                Paragraph::new(format!(
                    "Needs 80x24; current {}x{}\n0/q/Ctrl-C exit",
                    area.width, area.height
                )),
                area,
            );
            return;
        }
        let header = format!(
            "brrd-config  {}",
            self.menu
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    if i == self.section {
                        format!("[{} {}]", i + 1, s.short)
                    } else {
                        format!(" {} {} ", i + 1, s.short)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        );
        frame.render_widget(
            Paragraph::new(header),
            Rect::new(area.x, area.y, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(self.message.as_str()),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
        let section = &self.menu[self.section];
        let path = if self.page.is_some() {
            format!("{} > {}", section.name, section.items[self.selected].label)
        } else {
            section.name.to_owned()
        };
        frame.render_widget(
            Paragraph::new(path),
            Rect::new(area.x, area.y + 2, area.width, 1),
        );
        let body = Rect::new(area.x, area.y + 4, area.width, area.height - 5);
        if let Some(page) = &self.page {
            page.draw(frame, body);
        } else if section.items.is_empty() {
            frame.render_widget(
                Paragraph::new("Nothing is available in this version."),
                body,
            );
        } else {
            let text = section
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    format!(
                        "{} {}",
                        if i == self.selected { ">" } else { " " },
                        item.label
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            frame.render_widget(Paragraph::new(text), body);
        }
        let hints = self
            .bindings()
            .iter()
            .filter(|b| !b.hint.is_empty())
            .map(|b| b.hint)
            .collect::<Vec<_>>()
            .join(" | ");
        frame.render_widget(
            Paragraph::new(hints),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }
}
