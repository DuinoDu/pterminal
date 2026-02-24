/// Internal events for cross-module communication
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Terminal title changed
    TitleChanged(String),
    /// Bell received
    Bell,
    /// Terminal exited
    Exited,
    /// Request redraw
    Redraw,
}
