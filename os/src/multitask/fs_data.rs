use crate::filesystem::vfs::Path;

#[derive(Debug, Clone)]
pub struct FilesystemData {
    pub current_work_dir: Path,
}
