use defmt::*;
use defmt_rtt as _;

use embedded_sdmmc::{RawFile, RawVolume, VolumeIdx, VolumeManager};

use super::*;

pub struct AudioPlaybackState {
    pub volume: RawVolume,
    pub file: RawFile,
}

pub(super) async fn empty_handle_audio(request: AudioSdRequest) {
    match request {
        AudioSdRequest::Close => {}
        AudioSdRequest::Open(_, _) => {}
    }
}

pub(super) async fn handle_audio_request<
    'a,
    D,
    T,
    const DIRS: usize,
    const FILES: usize,
    const VOLS: usize,
>(
    request: AudioSdRequest,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
    playback_state: &mut Option<AudioPlaybackState>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        AudioSdRequest::Open(path, name) => {
            let result = (|| -> Result<u32, SdError> {
                let volume = volume_mgr.open_raw_volume(VolumeIdx(0)).unwrap();
                let root = volume_mgr.open_root_dir(volume).unwrap(); // TODO: HAndle these errors!

                let volume = volume.to_volume(volume_mgr);
                let root = root.to_directory(volume_mgr);

                // This is the new working directory
                let dir = open_dir_path(&volume_mgr, root, &path).unwrap();

                let file = dir
                    .open_file_in_dir(name.clone(), embedded_sdmmc::Mode::ReadOnly)
                    .unwrap();

                let mut header = [0u8; 44];
                if file.read(&mut header).unwrap() != 44 {
                    info!("[SD] File header less than 44 bytes");
                    return Err(embedded_sdmmc::Error::EndOfFile);
                }

                let data_size =
                    u32::from_le_bytes([header[40], header[41], header[42], header[43]]);

                // Clear it before if it was accdently left unclosed
                if let Some(mut state) = playback_state.take() {
                    match close(&mut state, volume_mgr) {
                        Ok(()) => {}                                // yay
                        Err(embedded_sdmmc::Error::BadHandle) => {} // Prob already closed
                        Err(_e) => {} // Otehr error i prob dont care about
                    }
                    // it should be dropped even if it errors because i said so
                }

                *playback_state = Some(AudioPlaybackState {
                    volume: volume.to_raw_volume(),
                    file: file.to_raw_file(),
                });
                Ok(data_size)
            })();

            loop {
                match AUDIO_FILLED.try_receive() {
                    Ok(buf) => AUDIO_EMPTY.send(buf).await,
                    Err(_e) => break,
                }
            }

            match result {
                Ok(size) => {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Opened {
                        data_offset: 44,
                        data_size: size,
                    });

                    while let Ok(buf) = AUDIO_EMPTY.try_receive()
                        && let Some(state) = playback_state
                    {
                        match volume_mgr.read(state.file, buf.as_mut_slice()) {
                            Ok(0) => {
                                AUDIO_EMPTY.send(buf).await;
                                close(state, &volume_mgr).unwrap();
                                *playback_state = None;
                                break;
                            }
                            Ok(n) => {
                                // Successful read
                                if n < AUDIO_CHUNK_BYTES {
                                    buf[n..].fill(0);
                                    info!("Trailing zeros in audio chunk, len {}", n);
                                }
                                AUDIO_FILLED.send(buf).await; // Hand off data to consumer
                            }
                            Err(_e) => {
                                AUDIO_EMPTY.send(buf).await;
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("[SD] audio open failed: {:?}", Debug2Format(&e));
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                }
            }
        }
        AudioSdRequest::Close => {
            // Send buffers back
            loop {
                match AUDIO_FILLED.try_receive() {
                    Ok(buf) => AUDIO_EMPTY.send(buf).await,
                    Err(_e) => break,
                }
            }

            if let Some(mut state) = playback_state.take() {
                if let Err(_e) = close(&mut state, volume_mgr) {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
                } else {
                    AUDIO_SD_RESPONSE.signal(AudioSdResponse::Closed);
                }
            } else {
                AUDIO_SD_RESPONSE.signal(AudioSdResponse::Error);
            }
        }
    }
}
