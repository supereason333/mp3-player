use defmt::*;
use defmt_rtt as _;

use core::fmt::Write;

use embedded_sdmmc::ShortFileName;

use embedded_graphics::{
    mono_font::MonoTextStyle, mono_font::ascii::FONT_5X7, pixelcolor::Rgb565, prelude::*,
    text::Text,
};

use crate::display::FbType;
use crate::input::NavEvent;
use crate::screens::browser::BrowserScreen;
use crate::screens::{FileOpenerScreen, Screen, ScreenLogic, Transition};

use crate::dac;
use crate::sd::{AUDIO_SD_REQUEST, AUDIO_SD_RESPONSE, AudioSdRequest, AudioSdResponse, DirPath};

pub struct PlayerScreen {
    opened_path: DirPath,
    opened_file: ShortFileName,
    status: heapless::String<32>,
}

impl PlayerScreen {
    pub fn new(path: DirPath, file: ShortFileName) -> Self {
        Self {
            opened_file: file,
            opened_path: path,
            status: heapless::String::new(),
        }
    }
}

impl ScreenLogic for PlayerScreen {
    async fn on_event(&mut self, event: NavEvent) -> Transition {
        match event {
            NavEvent::Select => Transition::GoTo(Screen::Browser(BrowserScreen::new_with_path(
                self.opened_path.clone(),
            ))),
            _ => Transition::Stay,
        }
    }

    async fn on_enter(&mut self) {
        let _ = self.status.push_str("Opening...");

        AUDIO_SD_REQUEST
            .send(AudioSdRequest::Open(
                self.opened_path.clone(),
                self.opened_file.clone(),
            ))
            .await;

        match AUDIO_SD_RESPONSE.wait().await {
            AudioSdResponse::Opened {
                data_offset,
                data_size,
            } => {
                info!("[Player] opened, {} bytes of PCM data", data_size);
                self.status.clear();
                let _ = core::write!(self.status, "Playing ({} bytes)", data_size);

                // stream and "play" chunks until EOF
                loop {
                    AUDIO_SD_REQUEST.send(AudioSdRequest::ReadChunk).await;
                    match AUDIO_SD_RESPONSE.wait().await {
                        AudioSdResponse::Chunk(pcm) => {
                            dac::write_samples(&pcm).await;
                        }
                        AudioSdResponse::Eof => {
                            info!("[Player] playback finished");
                            self.status.clear();
                            let _ = self.status.push_str("Done");
                            break;
                        }
                        AudioSdResponse::Error => {
                            error!("[Player] read error during playback");
                            self.status.clear();
                            let _ = self.status.push_str("Error");
                            break;
                        }
                        _ => break,
                    }
                }

                AUDIO_SD_REQUEST.send(AudioSdRequest::Close).await;
                let _ = AUDIO_SD_RESPONSE.wait().await;
            }
            _ => {
                error!("[Player] failed to open file");
                self.status.clear();
                let _ = self.status.push_str("Open failed");
            }
        }
    }

    fn draw(&self, fb: &mut FbType) {
        let style = MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN);

        let filename_text = match core::str::from_utf8(self.opened_file.base_name()) {
            Ok(s) => s,
            Err(_) => "‹invalid utf-8›",
        };

        Text::new(filename_text, Point::new(0, 6), style)
            .draw(fb)
            .unwrap();
        Text::new(&self.status, Point::new(0, 16), style)
            .draw(fb)
            .unwrap();
    }
}
