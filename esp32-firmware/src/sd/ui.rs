use heapless::Vec as HVec;

use embedded_sdmmc::{Volume, VolumeManager};

use super::*;
//larpus maximus
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
    volume: &Volume<'a, D, T, DIRS, FILES, VOLS>,
) where
    D: embedded_sdmmc::BlockDevice,
    T: embedded_sdmmc::TimeSource,
{
    match request {
        SdRequest::ListDir(path, from_idx) => {
            let mut guard_entries = DIRECTORY_LIST.lock().await;
            guard_entries.drain(..);

            let root = volume.open_root_dir().unwrap();
            let directory = open_dir_path(volume_mgr, root, &path).unwrap();

            let mut added = 0;
            let mut offset = 0;
            let mut more = false;
            directory
                .iterate_dir(|entry| {
                    if added == 32 {
                        return;
                    }
                    if added > 32 {
                        more = true;
                        return;
                    }
                    if offset < from_idx {
                        offset += 1;
                        return;
                    }

                    if entry.attributes.is_system()
                        || entry.attributes.is_volume()
                        || entry.attributes.is_hidden()
                    // || entry.attributes.is_archive()
                    {
                        return;
                    }
                    if entry.name.base_name()[..1] == *b"." || entry.name.base_name()[..1] == *b"_"
                    {
                        return;
                    }

                    let _ = guard_entries.push(entry.clone());

                    added += 1;
                })
                .unwrap();

            directory.close().unwrap();

            SD_RESPONSE.signal(SdResponse::DirLoaded(!more));
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
