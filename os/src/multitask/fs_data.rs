use crate::filesystem::vfs::DEntryArc;

#[derive(Debug)]
pub struct FilesystemData {
    pub current_work_dir: DEntryArc,
}
