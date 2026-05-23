use crate::prelude::*;

#[derive(Clone)]
pub struct Ext4MountPoint {
    /// Mount done flag.
    pub mounted: bool,
    /// Mount point name
    pub mount_name: Vec<u8>,
}
impl Ext4MountPoint {
    pub fn new(name: &[u8]) -> Self {
        Self {
            mounted: false,
            mount_name: name.to_vec(),
        }
    }
}
impl Debug for Ext4MountPoint {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "Ext4MountPoint {{ mount_name: {:?} }}", self.mount_name)
    }
}
