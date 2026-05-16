use core::ptr::NonNull;

use crate::{
    basic::sync::{OnceInited, RwLock},
    devices::BLOCK_DEVICES,
    memory::address::PhysAddr,
};
use alloc::boxed::Box;
use flat_device_tree::Fdt;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{Transport, mmio::MmioTransport},
};

pub fn scan_virtio_devices(fdt: &Fdt) {
    let vmmios = fdt.all_nodes().filter(|node| {
        node.compatible()
            .is_some_and(|v| v.all().any(|s| s == "virtio,mmio"))
    });
    for node in vmmios {
        let reg_region = node.reg().next().unwrap();
        let start_phy = reg_region.starting_address as usize;

        let start = PhysAddr(start_phy).to_virtual_address().0;
        let size = reg_region.size.unwrap_or_else(|| {
            log::warn!(
                "VirtIO 设备 {} 的内存区域大小读取失败，使用 0x1000 替代...",
                node.name
            );
            0x1000
        });

        // 注意，这里得到的 start + [0, size) 尽管确实是内存中的地址，但其实际并不在内存中（而是对应设备寄存器），
        // 不能按和普通内存一样的方法处理（虽然这里我们全委托给库了所以没什么影响）

        let Some(header) = NonNull::new(start as *mut _) else {
            log::warn!(
                "VirtIO 设备 {} 的内存区域为空指针，跳过后续步骤...",
                node.name
            );
            continue;
        };
        let transport = match unsafe { MmioTransport::new(header, size) } {
            Ok(transport) => transport,
            Err(err) => {
                log::warn!(
                    "VirtIO 设备 {} 读取失败：{err:?}，跳过后续步骤...",
                    node.name
                );
                continue;
            }
        };

        log::info!("VirtIO 设备 {}: {:?}", node.name, transport.device_type());

        use virtio_drivers::transport::DeviceType;

        match transport.device_type() {
            DeviceType::Network => log::error!("TODO: VirtIO 网络设备"),
            DeviceType::Block => {
                // FIXME: 现在这样不支持多块设备（虽然我们可能也没必要支持）
                log::warn!("初始化块设备，且将此块设备当作唯一的块设备");

                let device_box = Box::new(RwLock::new(VirtIOBlk::new(transport).unwrap()));

                OnceInited::init(&BLOCK_DEVICES, [Box::leak(device_box)]);
            }
            _ => log::warn!(
                "忽略暂不支持的 VirtIO 设备 {}（其类型为 {:?}）",
                node.name,
                transport.device_type()
            ),
        }
    }
}
