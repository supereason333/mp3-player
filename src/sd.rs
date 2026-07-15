use defmt::*;
use defmt_rtt as _;

use heapless::Vec as HVec;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::{
    Directory, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};

use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::Async;
use embassy_rp::spi::Spi;

use embedded_hal_bus::spi::ExclusiveDevice;

use embassy_time::Delay;

// Simple requests
pub enum SdRequest {
    ListDir(DirPath),
    ReadFile(DirPath, ShortFileName), // whole-file read, capped size, for text/hex viewers
}

pub enum SdResponse {
    DirListing(DirListing),
    FileContents(HVec<u8, 512>),
}

pub static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
pub static SD_RESPONSE: Signal<CriticalSectionRawMutex, SdResponse> = Signal::new();

// ---- Audio-facing (streaming playback) ----
// pub enum AudioSdRequest {
//     Open(DirPath, ShortFileName),
//     ReadChunk, // "give me the next chunk of the currently open file"
//     Close,
// }

// pub enum AudioSdResponse {
//     Opened { size: u32 },
//     Chunk(HVec<u8, 512>),
//     Eof,
//     Error,
// }

// pub static AUDIO_SD_REQUEST: Channel<CriticalSectionRawMutex, AudioSdRequest, 2> = Channel::new();
// pub static AUDIO_SD_RESPONSE: Signal<CriticalSectionRawMutex, AudioSdResponse> = Signal::new();

pub type DirListing = HVec<(ShortFileName, u32, bool), 32>;
pub type DirPath = HVec<ShortFileName, 16>;
type SdBlockDevice =
    SdCard<ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>, Delay>;
type SdError = embedded_sdmmc::Error<<SdBlockDevice as embedded_sdmmc::BlockDevice>::Error>;

/// Code from https://github.com/rp-rs/rp-hal-boards/blob/main/boards/rp-pico/examples/pico_spi_sd_card.rs
/// A dummy timesource, which is mostly important for creating files.
#[derive(Default)]
pub struct DummyTimesource();

impl TimeSource for DummyTimesource {
    // In theory you could use the RTC of the rp2040 here, if you had
    // any external time synchronizing device.
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 0,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

#[embassy_executor::task]
pub async fn sd_task(
    spi_device: ExclusiveDevice<Spi<'static, SPI0, Async>, Output<'static>, Delay>,
) -> ! {
    info!("[SD] SD task spawned");
    let sdcard = SdCard::new(spi_device, Delay);
    let sd_size = sdcard.num_bytes().expect("failed to get sdcard size");
    info!("[SD] card size is {} bytes", sd_size);

    let volume_mgr = VolumeManager::new(sdcard, DummyTimesource::default());

    loop {
        let request = SD_REQUEST.receive().await;
        match request {
            SdRequest::ListDir(path) => {
                let mut entries: DirListing = HVec::new();

                let result = (|| -> Result<(), SdError> {
                    let volume0 = volume_mgr.open_volume(VolumeIdx(0))?;
                    let root = volume0.open_root_dir()?;
                    let result = list_dir_recursive(&root, &path, &mut entries);
                    root.close()?;
                    volume0.close()?;
                    result
                })();

                if let Err(e) = result {
                    error!("SD list error: {:?}", Debug2Format(&e));
                }

                SD_RESPONSE.signal(SdResponse::DirListing(entries));
            }
            SdRequest::ReadFile(path, name) => {
                let mut buf: HVec<u8, 512> = HVec::new();

                let result = (|| -> Result<(), SdError> {
                    let volume0 = volume_mgr.open_volume(VolumeIdx(0))?;
                    let root = volume0.open_root_dir()?;

                    let r = with_dir_at_path(&root, &path, |target_dir| {
                        let file = target_dir
                            .open_file_in_dir(name.clone(), embedded_sdmmc::Mode::ReadOnly)?;
                        buf.resize_default(512).ok();
                        let bytes_read = file.read(&mut buf)?;
                        buf.truncate(bytes_read);
                        file.close()?;
                        Ok(())
                    });

                    root.close()?;
                    volume0.close()?;
                    r
                })();

                if let Err(e) = result {
                    error!("[SD] ReadFile failed: {:?}", Debug2Format(&e));
                }

                SD_RESPONSE.signal(SdResponse::FileContents(buf));
            }
        }
    }
}

fn with_dir_at_path<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize, F, R>(
    dir: &Directory<D, T, DIRS, FILES, VOLS>,
    path: &[ShortFileName],
    f: F,
) -> Result<R, embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
    F: FnOnce(&Directory<D, T, DIRS, FILES, VOLS>) -> Result<R, embedded_sdmmc::Error<D::Error>>,
{
    match path.split_first() {
        None => f(dir), // reached target — run the caller's logic here
        Some((first, rest)) => {
            let child = dir.open_dir(first)?;
            let result = with_dir_at_path(&child, rest, f);
            child.close()?;
            result
        }
    }
}

fn list_dir_recursive<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    dir: &Directory<D, T, DIRS, FILES, VOLS>,
    path: &[ShortFileName],
    entries: &mut DirListing,
) -> Result<(), embedded_sdmmc::Error<D::Error>>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    with_dir_at_path(dir, path, |target_dir| {
        target_dir.iterate_dir(|entry| {
            if entry.attributes.is_system()
                || entry.attributes.is_volume()
                || entry.name.base_name()[0] == b'_'
                || (entry.name.base_name().len() == 1 && entry.name.base_name()[0] == b'.')
            {
                return;
            }
            let _ = entries.push((
                entry.name.clone(),
                entry.size,
                entry.attributes.is_directory(),
            ));
        })
    })
}
