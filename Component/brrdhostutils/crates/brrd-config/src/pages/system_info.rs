use super::Page;
use crate::app::{Binding, Intent, Source};
use brrd_core::Snapshot;
use ratatui::{Frame, layout::Rect, widgets::Paragraph};

pub struct SystemInfo {
    source: Source,
    snapshot: Snapshot,
}
impl SystemInfo {
    pub fn new(source: Source) -> Self {
        let snapshot = source.borrow_mut().snapshot();
        Self { source, snapshot }
    }
}
impl Page for SystemInfo {
    fn tick(&mut self) {
        self.snapshot = self.source.borrow_mut().snapshot();
    }
    fn intent(&mut self, _intent: Intent) {}
    fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.snapshot.report_lines().join("\n")),
            area,
        );
    }
    fn bindings(&self) -> Vec<Binding> {
        vec![]
    }
    fn message(&self) -> Option<&str> {
        self.snapshot
            .has_unavailable()
            .then_some("System information could not be read.")
    }
}
