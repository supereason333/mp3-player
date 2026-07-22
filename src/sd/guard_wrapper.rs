use defmt::*;
use defmt_rtt as _;

use embassy_sync::pubsub::Error;
use embedded_hal_bus::spi::NoDelay;
use heapless::Vec as HVec;

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;

use embedded_sdmmc::{
    RawDirectory, RawFile, RawVolume, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx,
    VolumeManager,
};

use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi;
use embassy_rp::spi::Async;
use embassy_rp::spi::Spi;

use embedded_hal_bus::spi::ExclusiveDevice;

use embassy_time::Delay;
use embassy_time::Instant;

// Volume guard
struct RawVolumeGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    volume_mgr: &'a VolumeManager<D, T>,
    handle: Option<RawVolume>,
}

impl<'a, D, T> RawVolumeGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn new(volume_mgr: &'a VolumeManager<D, T>, handle: RawVolume) -> Self {
        Self {
            volume_mgr,
            handle: Some(handle),
        }
    }

    // call this once ownership is successfully handed off elsewhere
    fn release(mut self) -> RawVolume {
        self.handle.take().unwrap()
    }
}

impl<'a, D, T> Drop for RawVolumeGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            if let Err(e) = self.volume_mgr.close_volume(h) {
                error!("[SD] guard failed to close volume: {:?}", Debug2Format(&e));
            }
        }
    }
}

// raw directory guard
struct RawDirGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    volume_mgr: &'a VolumeManager<D, T>,
    handle: Option<RawDirectory>,
}

impl<'a, D, T> RawDirGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn new(volume_mgr: &'a VolumeManager<D, T>, handle: RawDirectory) -> Self {
        Self {
            volume_mgr,
            handle: Some(handle),
        }
    }

    // call this once ownership is successfully handed off elsewhere
    fn release(mut self) -> RawDirectory {
        self.handle.take().unwrap()
    }
}

impl<'a, D, T> Drop for RawDirGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            if let Err(e) = self.volume_mgr.close_dir(h) {
                error!("[SD] guard failed to close volume: {:?}", Debug2Format(&e));
            }
        }
    }
}

// raw file guard
struct RawFileGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    volume_mgr: &'a VolumeManager<D, T>,
    handle: Option<RawFile>,
}

impl<'a, D, T> RawFileGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn new(volume_mgr: &'a VolumeManager<D, T>, handle: RawFile) -> Self {
        Self {
            volume_mgr,
            handle: Some(handle),
        }
    }

    // call this once ownership is successfully handed off elsewhere
    fn release(mut self) -> RawFile {
        self.handle.take().unwrap()
    }
}

impl<'a, D, T> Drop for RawFileGuard<'a, D, T>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            if let Err(e) = self.volume_mgr.close_file(h) {
                error!("[SD] guard failed to close volume: {:?}", Debug2Format(&e));
            }
        }
    }
}
