use defmt::Str;
use heapless::String;

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::{Screen, ScreenLogic, Transition};

pub struct DialogueScreen {
    from_screen: Option<Screen>,
    message: String<64>,
}

impl DialogueScreen {
    pub fn new(initial: Screen, message: heapless::String<64>) -> Self {
        Self {
            from_screen: Some(initial),
            message,
        }
    }
}

impl ScreenLogic for DialogueScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match self.from_screen.take() {
            Some(screen) => Transition::GoTo(screen),
            None => Transition::Stay,
        }
    }

    fn draw(&self, fb: &mut FbType) {}
}
