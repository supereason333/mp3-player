use defmt::*;
use defmt_rtt as _;

use heapless::Vec as HVec;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::{SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager};

use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::Async;
use embassy_rp::spi::Spi;

use embedded_hal_bus::spi::ExclusiveDevice;

use embassy_time::Delay;

pub enum SdRequest {
    ListDir(HVec<u8, 64>), // path, or empty for root — encode however fits your dir stack
}

static SD_REQUEST: Channel<CriticalSectionRawMutex, SdRequest, 4> = Channel::new();
static SD_RESPONSE: Signal<CriticalSectionRawMutex, DirListing> = Signal::new();

pub type DirListing = HVec<(ShortFileName, u32, bool), 32>;

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
    let sdcard = SdCard::new(spi_device, Delay);

    info!("Init SD card controller and retrieve card size...");
    let sd_size = sdcard.num_bytes().expect("failed to get sdcard size");
    info!("card size is {} bytes", sd_size);

    let volume_mgr = VolumeManager::new(sdcard, DummyTimesource::default());
    let volume0 = volume_mgr
        .open_volume(VolumeIdx(0))
        .expect("failed to open volume");
    let mut root = match volume0.open_root_dir() {
        Ok(dir) => dir,
        Err(e) => {
            defmt::panic!("open_root_dir failed: {:?}", Debug2Format(&e));
        }
    };

    loop {
        let request = SD_REQUEST.receive().await;
        match request {
            SdRequest::ListDir(_path) => {
                let mut entries: DirListing = HVec::new();

                let result = (|| -> Result<(), embedded_sdmmc::Error<embassy_rp::spi::Error>> {
                    match root.iterate_dir(|entry| {
                        let _ = entries.push((
                            entry.name.clone(),
                            entry.size,
                            entry.attributes.is_directory(),
                        ));
                    }) {
                        Ok(()) => {}
                        Err(e) => {
                            defmt::panic!("root dir iterate dir error: {:?}", Debug2Format(&e))
                        }
                    }

                    Ok(())
                })();

                if let Err(e) = result {
                    error!("SD list error: {:?}", Debug2Format(&e));
                }

                SD_RESPONSE.signal(entries);
            }
        }
    }
}
