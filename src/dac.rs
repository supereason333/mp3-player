use defmt::*;

/// Placeholder for real DAC/I2S output. Just logs how much data "played."
pub async fn write_samples(pcm: &[u8]) {
    info!("[DAC] would write {} bytes of PCM", pcm.len());
    // simulates playback speed
    embassy_time::Timer::after_micros(50).await;
}
