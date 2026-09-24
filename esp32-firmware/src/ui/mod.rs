pub mod assets;

use defmt::*;
use defmt_rtt as _;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::mono_font::iso_8859_9::FONT_5X7;

use core::fmt::Write;

use heapless::String;
use heapless::Vec as HVec;

use embedded_graphics::Drawable;
use embedded_graphics::framebuffer::Framebuffer;
use embedded_graphics::image::Image;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::{Point, RgbColor};
use embedded_graphics::text::Text;
use tinybmp::{Bmp, ParseError};

use embassy_time::Duration;
use embassy_time::Timer;

use crate::dac;
use crate::display::*;
use crate::input;
use crate::input::NavEvent;
use crate::sd;
use crate::sd::DirPath;
use assets::*;

#[derive(defmt::Format)]
enum Screens {
    Main,
    Browser,
    Queue,
}

enum ScreenTransition {
    Main,
    Browser,
    Queue,
}

struct ScrollingText {
    text: String<32>,
    screen_pos: Point,
    space_width: i32,
    scroll_pos: i32,
    t: i32,
}

const SCROLL_INTERVAL: i32 = 1;
const PAUSE_DURATION: i32 = 5;

impl ScrollingText {
    fn new(text: &str, screen_pos: Point, space_width: i32) -> Self {
        let scroll_pos: i32 = if text.len() <= space_width as usize {
            -1
        } else {
            0
        };

        let mut truncated = String::<32>::new();
        let _ = truncated.push_str(&text[..text.len().min(36)]);

        Self {
            text: truncated,
            screen_pos,
            space_width,
            scroll_pos,
            t: 0,
        }
    }
    fn update(&mut self, dt: i32, fb: &mut FbType) {
        let text_style = MonoTextStyle::new(&FONT_4X6, Rgb565::WHITE);
        if self.scroll_pos == -1 {
            // Dosent need to scroll, just update text
            let name_str: &str = &self.text;
            Text::new(name_str, self.screen_pos, text_style.clone())
                .draw(fb)
                .unwrap();
            return;
        }

        let max_scroll = self.text.len() as i32 - self.space_width;
        self.t += dt;

        // Hold longer at the start and end positions, step normally in between
        let at_end = self.scroll_pos == 0 || self.scroll_pos == max_scroll;
        let threshold = if at_end {
            PAUSE_DURATION
        } else {
            SCROLL_INTERVAL
        };

        if self.t >= threshold {
            self.t = 0;
            self.scroll_pos += 1;
            if self.scroll_pos > max_scroll {
                self.scroll_pos = 0;
            }
        }

        let start = self.scroll_pos as usize;
        let end = (self.scroll_pos + self.space_width) as usize;
        let end = end.min(self.text.len());
        let str: &str = self.text.get(start..end).unwrap_or("");
        Text::new(str, self.screen_pos, text_style.clone())
            .draw(fb)
            .unwrap();
    }
    fn set_text(&mut self, text: &str) {
        let mut truncated = String::<32>::new();
        let _ = truncated.push_str(&text[..text.len().min(32)]);
        self.text = truncated;
        self.scroll_pos = if self.text.len() <= self.space_width as usize {
            -1
        } else {
            0
        };
        self.t = 0;
    }

    fn reset_position(&mut self) {
        self.scroll_pos = if self.text.len() <= self.space_width as usize {
            -1
        } else {
            0
        };
        self.t = 0;
    }
}

#[embassy_executor::task]
pub async fn ui_task(mut display: Display) {
    info!("[UI] UI Task spawned");
    let mut fb: FbType = Framebuffer::new();
    let mut current_screen = Screens::Browser;

    let mut main_screen = MainScreen::new();
    let mut browser_screen = BrowserScreen::new();
    let mut queue_screen = QueueScreen::new();

    loop {
        let transition = match current_screen {
            Screens::Main => main_screen.start(&mut fb).await,
            Screens::Browser => browser_screen.start(&mut fb, &mut display).await,
            Screens::Queue => queue_screen.start(&mut fb).await,
        };

        match transition {
            ScreenTransition::Main => current_screen = Screens::Main,
            ScreenTransition::Browser => current_screen = Screens::Browser,
            ScreenTransition::Queue => current_screen = Screens::Queue,
            _ => {}
        }
    }
}

struct MainScreen {}

impl MainScreen {
    fn new() -> Self {
        Self {}
    }

    async fn start(&mut self, fb: &mut FbType) -> ScreenTransition {
        ScreenTransition::Browser
    }
}

const BROWSER_DISPLAY_ITEMS: usize = 16;

struct BrowserScreen {
    working_dir: DirPath,
    /// selected item in range of current display items
    selected: usize,
    /// Offset in terms of BROWSER_DISPLAY_ITEMS
    offset: usize,
    at_end: bool,
    last_action: BrowserAction,
}

/// stuff that oculd have happened
/// use to draw right stuff
/// TODO: Impliment Err(ErrorType)
enum BrowserAction {
    QueueAdded,
    QueueAddErr,
    Pause,
    Resume,
    Started,
    PlayErr,
    SelectError,
}

impl BrowserScreen {
    fn new() -> Self {
        Self {
            last_action: BrowserAction::Started,
            working_dir: HVec::new(),
            selected: 0,
            offset: 0,
            at_end: false,
        }
    }

    async fn start(&mut self, fb: &mut FbType, disp: &mut Display) -> ScreenTransition {
        match sd::client::ui_list_dir(
            self.working_dir.clone(),
            self.offset * BROWSER_DISPLAY_ITEMS,
        )
        .await
        {
            Ok(end) => self.at_end = end,
            Err(e) => self.at_end = true,
        }

        self.draw(fb).await;
        disp.write_framebuf(fb).await;

        loop {
            let mut update_file_buf = false;
            match input::NAV_EVENT.wait().await {
                NavEvent::Up => {
                    self.selected += 1;
                    if self.selected >= BROWSER_DISPLAY_ITEMS && !self.at_end {
                        self.offset += 1;
                        self.selected = 0;
                        update_file_buf = true;
                    } else {
                        self.selected = self.selected.clamp(0, BROWSER_DISPLAY_ITEMS - 1);
                    }
                }
                NavEvent::Down => {
                    if self.selected == 0 && self.offset > 0 {
                        self.offset -= 1;
                        self.selected = BROWSER_DISPLAY_ITEMS - 1;
                        update_file_buf = true;
                    } else {
                        self.selected = self.selected.saturating_sub(1);
                        self.selected = self.selected.clamp(0, BROWSER_DISPLAY_ITEMS - 1);
                    }
                }
                NavEvent::Select => self.on_select().await,
                NavEvent::Back => {
                    if self.working_dir.pop() == None {
                        return ScreenTransition::Main;
                    }
                    self.offset = 0;
                    self.selected = 0;
                    match sd::client::ui_list_dir(
                        self.working_dir.clone(),
                        self.offset * BROWSER_DISPLAY_ITEMS,
                    )
                    .await
                    {
                        Ok(end) => self.at_end = end,
                        Err(()) => {}
                    }
                    self.print_path();
                }
                NavEvent::Play => self.add_queue().await,
                _ => continue,
            }
            if update_file_buf {
                match sd::client::ui_list_dir(
                    self.working_dir.clone(),
                    self.offset * BROWSER_DISPLAY_ITEMS,
                )
                .await
                {
                    Ok(end) => self.at_end = end,
                    Err(e) => self.at_end = true,
                }
            }
            self.draw(fb).await;
            disp.write_framebuf(fb).await;
            Timer::after(Duration::from_millis(10)).await;
        }
    }

    /// Selected track, add to queue
    async fn add_queue(&mut self) {
        let guard = sd::DIRECTORY_LIST.lock().await;
        let file = guard.get(self.selected);
        let filename = if let Some(f) = file {
            f.name.clone()
        } else {
            self.last_action = BrowserAction::QueueAddErr;
            return;
        };
        if filename.extension() != b"WAV" && filename.extension() != b"MP3" {
            return;
        }
        match dac::client::queue_add(self.working_dir.clone(), filename).await {
            Ok(()) => {
                self.last_action = BrowserAction::QueueAdded;
            }
            Err(e) => {
                self.last_action = BrowserAction::QueueAddErr;
            }
        }
    }

    /// Specificly start playing that track
    async fn on_select(&mut self) {
        let guard = sd::DIRECTORY_LIST.lock().await;
        let file = guard.get(self.selected).cloned();
        // Drops otherwise SD task cant get lock
        core::mem::drop(guard);
        let file = if let Some(f) = file {
            f
        } else {
            self.last_action = BrowserAction::SelectError;
            return;
        };
        if file.name.extension() != b"WAV" && file.name.extension() != b"MP3" {
            if file.attributes.is_directory() {
                self.working_dir.push(file.name.clone()).ok();
                self.selected = 0;
                self.offset = 0;
                match sd::client::ui_list_dir(
                    self.working_dir.clone(),
                    self.offset * BROWSER_DISPLAY_ITEMS,
                )
                .await
                {
                    Ok(end) => self.at_end = end,
                    Err(()) => {}
                }
                self.print_path();
            }
            return;
        }
        match dac::client::start_playback(self.working_dir.clone(), file.name.clone()).await {
            Ok(()) => self.last_action = BrowserAction::Started,
            Err(e) => self.last_action = BrowserAction::PlayErr,
        }
    }

    /// resets the file to be root and position 0
    fn reset(&mut self) {
        self.working_dir.drain(..);
        self.offset = 0;
        self.selected = 0;
    }

    // Draws whatever is inside of DIRECTORY_LIST
    async fn draw(&mut self, fb: &mut FbType) {
        fb.clear(Rgb565::BLACK);
        let guard = sd::DIRECTORY_LIST.lock().await;

        let (offset_x, offset_y) = (5, 20);
        let line_offset = 10;

        for (i, entry) in guard.iter().enumerate() {
            let style = if i == self.selected {
                MonoTextStyle::new(&FONT_5X7, Rgb565::GREEN)
            } else {
                MonoTextStyle::new(&FONT_5X7, Rgb565::WHITE)
            };

            let mut buf: String<16> = String::new();
            if entry.attributes.is_directory() {
                core::write!(buf, "{}/", entry.name).ok();
            } else {
                core::write!(buf, "{}", entry.name).ok();
            }

            let name: &str = &buf;

            Text::new(
                name,
                Point::new(offset_x, (offset_y + i * line_offset) as i32),
                style,
            )
            .draw(fb)
            .unwrap();

            if i >= BROWSER_DISPLAY_ITEMS {
                break;
            }
        }
    }

    fn print_path(&self) {
        info!("------ WORKING DIRECTORY ------");
        for dir in &self.working_dir {
            let mut buf: String<16> = String::new();
            core::write!(buf, "{}/", dir).ok();
            info!("/{}", &buf as &str);
        }
        info!("-------------------------------");
    }
}

struct QueueScreen {}

impl QueueScreen {
    fn new() -> Self {
        Self {}
    }

    async fn start(&mut self, fb: &mut FbType) -> ScreenTransition {
        ScreenTransition::Browser
    }
}

fn draw_ui_bg(fb: &mut FbType, screen: &Screens) -> Result<(), ParseError> {
    let bmp = match screen {
        Screens::Main => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
        Screens::Browser => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
        Screens::Queue => Bmp::<embedded_graphics::pixelcolor::Rgb565>::from_slice(MAIN_UI_BG),
    }?;
    let image = Image::new(&bmp, Point::zero());
    image.draw(fb).unwrap();
    Ok(())
}
