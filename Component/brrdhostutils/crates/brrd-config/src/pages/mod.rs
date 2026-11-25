mod system_info;
use crate::app::{Binding, Intent, Source};
use ratatui::{Frame, layout::Rect};

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

pub fn system_info(source: Source) -> Box<dyn Page> {
    Box::new(system_info::SystemInfo::new(source))
}
