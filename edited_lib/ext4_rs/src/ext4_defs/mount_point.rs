use core::bstr::ByteStr;

use alloc::bstr::ByteString;

use crate::prelude::*;

#[derive(Clone)]
pub struct Ext4MountPoint {
    /// Mount done flag.
    pub mounted: bool,
    /// Mount point name
    pub mount_name: ByteString,
}
impl Ext4MountPoint {
    pub fn new(name: &ByteStr) -> Self {
        Self {
            mounted: false,
            mount_name: ByteString::from(name),
        }
    }
}
impl Debug for Ext4MountPoint {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "Ext4MountPoint {{ mount_name: {:?} }}", self.mount_name)
    }
}
