use crate::app::{Binding, Intent, Source};
use ratatui::{Frame, layout::Rect, widgets::Paragraph};

/// Pages own their content and behavior; the frame only supplies an area.
pub trait Page {
    fn tick(&mut self);
    fn intent(&mut self, intent: Intent);
    fn draw(&self, frame: &mut Frame, area: Rect);
    fn bindings(&self) -> Vec<Binding>;
    fn message(&self) -> Option<&str> {
        None
    }
}

pub fn system_info(_source: Source) -> Box<dyn Page> {
    Box::new(Pending)
}
struct Pending;
impl Page for Pending {
    fn tick(&mut self) {}
    fn intent(&mut self, _intent: Intent) {}
    fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Paragraph::new("System Info"), area);
    }
    fn bindings(&self) -> Vec<Binding> {
        vec![]
    }
}
