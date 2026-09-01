use defmt::*;
use defmt_rtt as _;

use embedded_sdmmc::ShortFileName;

use heapless::Vec as HVec;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::browser::BrowserScreen;
use crate::screens::{FileOpenerScreen, Screen, ScreenLogic, Transition};
use crate::sd::*;

pub struct CatScreen {
    opened_path: DirPath,
    opened_file: ShortFileName,
    file_data: HVec<u8, 512>,
    scroll_offset: usize,
}

impl CatScreen {
    pub fn new(path: DirPath, file: ShortFileName) -> Self {
        CatScreen {
            opened_path: path,
            opened_file: file,
            file_data: HVec::new(),
            scroll_offset: 0,
        }
    }
}

impl ScreenLogic for CatScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Up | NavEvent::For => {
                // info!("Up");
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            NavEvent::Down | NavEvent::Rev => {
                // info!("Down");
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            NavEvent::Select => {
                return Transition::GoTo(Screen::Browser(BrowserScreen::new_with_path(
                    self.opened_path.clone(),
                )));
            }
            _ => {}
        }
        Transition::Stay
    }

    fn draw(&self, fb: &mut FbType) {
        let style = MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN);

        let text = match core::str::from_utf8(&self.file_data) {
            Ok(s) => s,
            Err(_) => "‹invalid utf-8›",
        };

        const CHARS_PER_LINE: usize = 25; // 128px / 5px per FONT_5X7 char
        const LINE_HEIGHT: i32 = 8;
        const MAX_LINES: i32 = 160 / LINE_HEIGHT;

        let mut y = 0;
        for line in text.lines().skip(self.scroll_offset) {
            // further wrap long lines into CHARS_PER_LINE-sized chunks
            let mut remaining = line;
            while !remaining.is_empty() {
                if y >= MAX_LINES {
                    break;
                }
                let chunk_len = remaining.len().min(CHARS_PER_LINE);
                let (chunk, rest) = remaining.split_at(chunk_len);

                Text::new(chunk, Point::new(2, y * LINE_HEIGHT + 6), style)
                    .draw(fb)
                    .unwrap();

                remaining = rest;
                y += 1;
            }
            if line.is_empty() {
                y += 1; // blank line still advances
            }
        }
    }

    async fn on_enter(&mut self) {
        info!("Cat screen on enter");
        // info!("[Cat] Send SdRequest");
        // SD_REQUEST
        //     .send(SdRequest::ReadFile(
        //         self.opened_path.clone(),
        //         self.opened_file.clone(),
        //     ))
        //     .await;
        // self.file_data = match SD_RESPONSE.wait().await {
        //     SdResponse::FileContents(data) => data,
        //     SdResponse::DirListing(_) => {
        //         error!("[Cat] Got DirListing when expecting FileContents");
        //         HVec::new() // degrade to empty listing rather than crashing
        //     }
        // };
        // info!("[Cat] Recieved Sd Response");
    }
}
impl FileOpenerScreen for CatScreen {
    fn filename(&self) -> &embedded_sdmmc::ShortFileName {
        self.opened_path.last().unwrap()
    }
    fn return_path(&self) -> &DirPath {
        &self.opened_path
    }
}
