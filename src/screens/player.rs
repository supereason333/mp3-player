use embedded_sdmmc::ShortFileName;

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::browser::BrowserScreen;
use crate::screens::{FileOpenerScreen, Screen, ScreenLogic, Transition};
use crate::sd::DirPath;

pub struct PlayerScreen {
    opened_path: DirPath,
    opened_file: ShortFileName,
}

impl PlayerScreen {
    pub fn new(path: DirPath, file: ShortFileName) -> Self {
        Self {
            opened_file: file,
            opened_path: path,
        }
    }
}

impl ScreenLogic for PlayerScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        return Transition::GoTo(Screen::Browser(BrowserScreen::new_with_path(
            self.opened_path.clone(),
        )));
    }

    async fn on_enter(&mut self) {}

    fn draw(&self, fb: &mut FbType) {}
}

impl FileOpenerScreen for PlayerScreen {
    fn filename(&self) -> &ShortFileName {
        &self.opened_file
    }
    fn return_path(&self) -> &DirPath {
        &self.opened_path
    }
}
