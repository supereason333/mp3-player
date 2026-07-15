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

pub enum SdRequest {
    ListDir(DirPath), // path, or empty for root — encode however fits your dir stack
}

pub static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
pub static SD_RESPONSE: Signal<CriticalSectionRawMutex, DirListing> = Signal::new();

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

                SD_RESPONSE.signal(entries);
            }
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
    match path.split_first() {
        None => {
            // reached the target directory — list it
            // First filter out unwanted system and MacOS system files
            dir.iterate_dir(|entry| {
                if entry.attributes.is_system()
                    || entry.attributes.is_volume()
                    || entry.name.base_name()[0] == b'_'
                {
                    return;
                }
                if entry.name.base_name().len() == 1 {
                    if entry.name.base_name()[0] == b'.' {
                        return;
                    }
                }
                let _ = entries.push((
                    entry.name.clone(),
                    entry.size,
                    entry.attributes.is_directory(),
                ));
            })?;
            Ok(())
        }
        Some((first, rest)) => {
            let child = dir.open_dir(first)?;
            let result = list_dir_recursive(&child, rest, entries);
            child.close()?;
            result
        }
    }
}
