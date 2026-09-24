use embedded_sdmmc::File;

#[derive(Debug)]
pub enum WavParseError {
    WrongFormat,
    FileTooShort,
    ReadError,
}

#[derive(Debug, Clone)]
pub struct WavHeader {
    pub channels: u16,
    pub sample_rate: u32,
    pub audio_format: u16,
    pub bit_depth: u16,
    pub data_size: u32,
}

/// Parses a canonical 44-byte WAV header (RIFF/WAVE, single `fmt ` chunk
/// immediately followed by `data`, no extra chunks in between). Leaves the
/// file cursor positioned at the start of the PCM audio data on success.
pub(super) fn parse_wav_header<D, T, const DIRS: usize, const FILES: usize, const VOLS: usize>(
    file: &File<D, T, DIRS, FILES, VOLS>,
) -> Result<WavHeader, WavParseError>
where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    let mut header = [0u8; 44];
    file.seek_from_start(0)
        .map_err(|_| WavParseError::ReadError)?;
    let n = file
        .read(&mut header)
        .map_err(|_| WavParseError::ReadError)?;
    if n != 44 {
        return Err(WavParseError::FileTooShort);
    }

    // // Bytes 1-4: "RIFF"
    // if &header[0..4] != b"RIFF" {
    //     return Err(WavParseError::WrongFormat);
    // }
    // // Bytes 9-12: "WAVE"
    // if &header[8..12] != b"WAVE" {
    //     return Err(WavParseError::WrongFormat);
    // }
    // // Bytes 13-16: "fmt "
    // if &header[12..16] != b"fmt " {
    //     return Err(WavParseError::WrongFormat);
    // }
    // // Bytes 37-40: "data"
    // if &header[36..40] != b"data" {
    //     return Err(WavParseError::WrongFormat);
    // }

    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bit_depth = u16::from_le_bytes([header[34], header[35]]);
    let data_size = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);

    // Header is exactly 44 bytes in the canonical layout this function
    // assumes, so the read above already left the cursor at the start of
    // the data chunk — this seek is redundant in that case, but makes the
    // function correct regardless of the file cursor's position on entry.
    file.seek_from_start(44)
        .map_err(|_| WavParseError::ReadError)?;

    Ok(WavHeader {
        channels,
        sample_rate,
        audio_format,
        bit_depth,
        data_size,
    })
}
