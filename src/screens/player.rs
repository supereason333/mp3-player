use embedded_sdmmc::ShortFileName;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

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

    fn draw(&self, fb: &mut FbType) {
        let style = MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN);

        let text = match core::str::from_utf8(&self.filename().base_name()) {
            Ok(s) => s,
            Err(_) => "‹invalid utf-8›",
        };

        Text::new(text, Point::new(0, 6), style).draw(fb).unwrap();
    }
}

impl FileOpenerScreen for PlayerScreen {
    fn filename(&self) -> &ShortFileName {
        &self.opened_file
    }
    fn return_path(&self) -> &DirPath {
        &self.opened_path
    }
}
