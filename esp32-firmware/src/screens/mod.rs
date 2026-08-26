// screens/mod.rs
pub mod browser;
pub mod cat;
pub mod player;
pub mod screen_task;
pub mod settings;

use embedded_sdmmc::ShortFileName;

use crate::{display::FbType, input::NavEvent, sd::sd_task::DirPath};

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

    /// Called jus tbefore the screen closes, for cleaning up and other logic
    async fn on_close(&mut self) {}
}

pub trait FileOpenerScreen {
    /// Used to see what path this was opened from, so browser
    /// can be rebuilt correctly
    fn return_path(&self) -> &DirPath;
    fn filename(&self) -> &ShortFileName;
}

pub enum Screen {
    Browser(browser::BrowserScreen),
    Settings(settings::SettingsScreen),
    Cat(cat::CatScreen),
    // Dialogue(dialogue::DialogueScreen),
    Player(player::PlayerScreen),
}

impl Screen {
    pub async fn on_event(&mut self, event: NavEvent) -> Transition {
        match self {
            Screen::Browser(s) => s.on_event(event).await,
            Screen::Settings(s) => s.on_event(event).await,
            Screen::Cat(s) => s.on_event(event).await,
            // Screen::Dialogue(s) => s.on_event(event).await,
            Screen::Player(s) => s.on_event(event).await,
        }
    }

    pub fn draw(&self, fb: &mut FbType) {
        match self {
            Screen::Browser(s) => s.draw(fb),
            Screen::Settings(s) => s.draw(fb),
            Screen::Cat(s) => s.draw(fb),
            // Screen::Dialogue(s) => s.draw(fb),
            Screen::Player(s) => s.draw(fb),
        }
    }

    pub async fn on_enter(&mut self) {
        match self {
            Screen::Browser(s) => s.on_enter().await,
            Screen::Settings(s) => s.on_enter().await,
            Screen::Cat(s) => s.on_enter().await,
            // Screen::Dialogue(s) => s.on_enter().await,
            Screen::Player(s) => s.on_enter().await,
        }
    }

    pub async fn on_close(&mut self) {
        match self {
            Screen::Browser(s) => s.on_close().await,
            Screen::Settings(s) => s.on_close().await,
            Screen::Cat(s) => s.on_close().await,
            Screen::Player(s) => s.on_close().await,
        }
    }
}
