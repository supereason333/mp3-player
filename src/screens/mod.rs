// screens/mod.rs
pub mod browser;
pub mod settings;

use crate::{
    display::FbType,
    input::{NAV_EVENT, NavEvent},
};

pub enum Transition {
    Stay,
    GoTo(Screen),
}

pub trait ScreenLogic {
    /// Handle a nav event, mutate own state, optionally request a screen change
    async fn on_event(&mut self, event: NavEvent) -> Transition;

    /// Draw current state into the framebuffer
    fn draw(&self, fb: &mut FbType);

    /// Called once when this screen becomes active (e.g. to kick off an SD request)
    async fn on_enter(&mut self) {}
}

pub enum Screen {
    Browser(browser::BrowserScreen),
    Settings(settings::SettingsScreen),
}

impl Screen {
    pub async fn on_event(&mut self, event: NavEvent) -> Transition {
        match self {
            Screen::Browser(s) => s.on_event(event).await,
            Screen::Settings(s) => s.on_event(event).await,
        }
    }

    pub fn draw(&self, fb: &mut FbType) {
        match self {
            Screen::Browser(s) => s.draw(fb),
            Screen::Settings(s) => s.draw(fb),
        }
    }

    pub async fn on_enter(&mut self) {
        match self {
            Screen::Browser(s) => s.on_enter().await,
            Screen::Settings(s) => s.on_enter().await,
        }
    }
}
