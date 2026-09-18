use defmt::*;
use defmt_rtt as _;

use embassy_time::{Duration, Timer};

#[embassy_executor::task]
pub async fn decoder_task() {
    info!("[DECODER] Decoder task spawned");
    loop {}
}
