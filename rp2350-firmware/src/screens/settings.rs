use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::{ScreenLogic, Transition};

pub struct SettingsScreen {/* ... */}
impl ScreenLogic for SettingsScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        Transition::Stay
    }

    fn draw(&self, fb: &mut FbType) {}
}
