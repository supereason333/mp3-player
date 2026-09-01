use defmt::*;
use defmt_rtt as _;

use core::fmt::Write;

use heapless::String;
use heapless::Vec as HVec;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::cat::CatScreen;
use crate::screens::player::PlayerScreen;
use crate::screens::{Screen, ScreenLogic, Transition};

use crate::sd::*;

#[derive(Clone)]
pub struct BrowserScreen {
    entries: DirListing,
    selected_index: usize,
    path: DirPath,
}

impl BrowserScreen {
    pub fn new() -> Self {
        Self {
            entries: HVec::new(),
            selected_index: 0,
            path: HVec::new(),
        }
    }
    pub fn new_with_path(path: DirPath) -> Self {
        Self {
            entries: HVec::new(),
            selected_index: 0,
            path: path,
        }
    }
}

impl ScreenLogic for BrowserScreen {
    async fn on_enter(&mut self) {
        info!("[Browser] Send SdRequest");
        SD_REQUEST.send(SdRequest::ListDir(self.path.clone())).await;
        self.entries = match SD_RESPONSE.wait().await {
            SdResponse::DirListing(entries) => entries,
            SdResponse::FileContents(_) => {
                error!("[Browser] Got FileContents when expecting DirListing");
                HVec::new() // degrade to empty listing rather than crashing
            }
        };
        info!("[Browser] Recieved Sd Response");
        self.selected_index = 0;
    }

    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Up | NavEvent::For => {
                // info!("Up");
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            NavEvent::Down | NavEvent::Rev => {
                // info!("Down");
                self.selected_index =
                    (self.selected_index + 1).min(self.entries.len().saturating_sub(1));
            }
            NavEvent::Select => {
                // info!("Select");

                let mut buf: String<16> = String::new();
                let _ = core::write!(buf, "{}", self.entries[self.selected_index].0);
                let name: &str = &buf;
                info!("Selected {:?}", name);

                let entry = &self.entries[self.selected_index];
                if entry.2 {
                    // Is directory
                    let mut up_one = false;
                    if entry.0.base_name().len() == 2 {
                        if entry.0.base_name()[0..2] == [b'.', b'.'] {
                            // Selected up one dir
                            self.path.pop();
                            up_one = true;
                        }
                    }
                    if !up_one {
                        self.path
                            .push(self.entries[self.selected_index].0.clone())
                            .unwrap();
                    }
                    info!("[Browser] Send SdRequest");
                    SD_REQUEST.send(SdRequest::ListDir(self.path.clone())).await;
                    self.entries = match SD_RESPONSE.wait().await {
                        SdResponse::DirListing(entries) => entries,
                        SdResponse::FileContents(_) => {
                            error!("[Browser] Got FileContents when expecting DirListing");
                            HVec::new() // degrade to empty listing rather than crashing
                        }
                    };
                    info!("[Browser] Recieved Sd Response");
                    self.selected_index = 0;
                } else {
                    // Open normal file
                    let ext = entry.0.extension();
                    return match ext {
                        b"TXT" | b"BIN" | b"PLI" | b"LOG" | b"CFG" | b"RS" => Transition::GoTo(
                            Screen::Cat(CatScreen::new(self.path.clone(), entry.0.clone())),
                        ),
                        b"WAV" | b"MP3" => Transition::GoTo(Screen::Player(PlayerScreen::new(
                            self.path.clone(),
                            entry.0.clone(),
                        ))),
                        _ => Transition::Stay,
                    };
                }
            }
            _ => {}
        }
        Transition::Stay
    }

    async fn on_close(&mut self) {}

    fn draw(&self, fb: &mut FbType) {
        for (i, (name, _size, is_dir)) in self.entries.iter().enumerate() {
            let style;
            if i == self.selected_index {
                style = MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN);
            } else {
                style = MonoTextStyle::new(&FONT_5X7, Rgb565::WHITE)
            }

            let mut buf: String<16> = String::new(); // 8.3 + dot + null-ish headroom = "XXXXXXXX.XXX" = 12 chars, 16 is safe
            if is_dir.clone() {
                let _ = core::write!(buf, "{}/", name);
            } else {
                let _ = core::write!(buf, "{}", name);
            }
            // buf is heapless::String<16>, which derefs to &str:
            let name: &str = &buf;

            Text::new(name, Point::new(0, 8 * i as i32 + 6), style)
                .draw(fb)
                .unwrap();

            // info!("File {}", name);

            // let style = PrimitiveStyleBuilder::new()
            //     .stroke_color(Rgb565::CSS_DARK_SLATE_GRAY)
            //     .stroke_width(3)
            //     .fill_color(Rgb565::CSS_DARK_GRAY)
            //     .build();

            // Rectangle::new(Point::new(1, 1), Size::new(126, 158))
            //     .into_styled(style)
            //     .draw(&mut fb)
            //     .unwrap();
        }
    }
}
