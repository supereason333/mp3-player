use heapless::Vec as HVec;

use embedded_sdmmc::{VolumeIdx, VolumeManager};

use super::*;

// Basicaly if SD card does not exist it just returns empty shit
pub(super) async fn empty_handle_ui(request: SdRequest) {
    match request {
        SdRequest::ListDir(_) => {
            let empty: DirListing = HVec::new();
            SD_RESPONSE.signal(SdResponse::DirListing(empty));
        }
        SdRequest::ReadFile(_, _) => {
            let empty: HVec<u8, 512> = HVec::new();
            SD_RESPONSE.signal(SdResponse::FileContents(empty));
        }
    }
}

pub(super) async fn handle_ui_request<
    'a,
    D,
    T,
    const DIRS: usize,
    const FILES: usize,
    const VOLS: usize,
>(
    request: SdRequest,
    volume_mgr: &'a VolumeManager<D, T, DIRS, FILES, VOLS>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        SdRequest::ListDir(path) => {
            let mut entries: DirListing = HVec::new();
            let volume = volume_mgr.open_volume(VolumeIdx(0)).unwrap();
            let root = volume.open_root_dir().unwrap();
            let directory = open_dir_path(volume_mgr, root, &path).unwrap();

            directory
                .iterate_dir(|entry| {
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
                .unwrap();

            directory.close().unwrap();
            volume.close().unwrap();

            SD_RESPONSE.signal(SdResponse::DirListing(entries));
        }
        SdRequest::ReadFile(_path, _name) => {
            let buf: HVec<u8, 512> = HVec::new();
            // let buf = [0u8; 512];

            // TEMPORARYLY NOT USED REMOVED, WRITE AGAIN LATER!
            // Just retunrs empty data
            //
            // Use static buffer maybe?

            SD_RESPONSE.signal(SdResponse::FileContents(buf));
        }
    }
}
