use alloc::boxed::Box;
use flat_device_tree::{Fdt, node::FdtNode, standard_nodes::MemoryRange};
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType,
        pci::{
            PciTransport,
            bus::{BarInfo, Cam, Command, MemoryBarType, MmioCam, PciRoot},
            virtio_device_type,
        },
    },
};

use crate::{
    basic::sync::{OnceInited, RwLock},
    devices::{BLOCK_DEVICES, hal::VirtIOHal},
    memory::address::PhysAddr,
};

pub fn scan_pci_devices(fdt: &Fdt) {
    if let Some(pci_node) = fdt.find_compatible(&["pci-host-cam-generic"]) {
        log::debug!("PCI: {}", pci_node.name);
        do_scan(pci_node, Cam::MmioCam);
    }

    if let Some(pcie_node) = fdt.find_compatible(&["pci-host-ecam-generic"]) {
        log::debug!("PCIE: {}", pcie_node.name);
        do_scan(pcie_node, Cam::Ecam);
    }
}

fn do_scan(node: FdtNode, cam: Cam) {
    assert!(
        node.property("dma-coherent").is_some(),
        "不支持硬件不维护一致性的设备"
    );

    let Some(range) = select_pci_range_32(node.ranges()) else {
        log::warn!(
            "未能在 PCI 设备 {} 上找到合适的 32 位内存范围，跳过后续步骤...",
            node.name
        );
        return;
    };

    log::debug!(
        "为 PCI 设备 {} 选择的内存范围：[{:#x}, {:#x})",
        node.name,
        range.0,
        range.1
    );
    let mut bars_allocator = BarsBumpAllocator {
        current: range.0,
        last: range.1,
    };

    for reg_region in node.reg() {
        let start_phy = reg_region.starting_address as usize;

        let start = PhysAddr(start_phy).to_virtual_address().0;
        let Some(size) = reg_region.size else {
            log::warn!(
                "PCI 设备 {} 的从 {start_phy:#x} 开始的内存区域大小读取失败，跳过后续步骤...",
                node.name
            );
            continue;
        };

        log::debug!(
            "PCI 设备 {}: [{:#x}, {:#x})",
            node.name,
            start_phy,
            start_phy + size
        );

        let mut pci_root = PciRoot::new(unsafe { MmioCam::new(start as *mut u8, cam) });

        for bus_id in [0] {
            for (dev_pos, info) in pci_root.enumerate_bus(bus_id) {
                log::info!("在 {} {dev_pos} 上发现设备：{info}", node.name);

                let handle_func = match virtio_device_type(&info) {
                    Some(DeviceType::Block) => handle_block_device,
                    Some(DeviceType::Network) => handle_network_device,
                    Some(vd_type) => {
                        log::info!(
                            "设备 {info}（位于 {} {dev_pos}）的类型 {vd_type:?} 暂不支持，跳过后续步骤...",
                            node.name
                        );
                        continue;
                    }
                    None => {
                        log::warn!(
                            "设备 {info}（位于 {} {dev_pos}）的类型无法识别，跳过后续步骤...",
                            node.name
                        );
                        continue;
                    }
                };

                for (index, bar) in pci_root
                    .bars(dev_pos)
                    .unwrap()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(idx, bar)| bar.map(|bar| (idx as u8, bar)))
                {
                    log::trace!("#{index} BAR {bar:?}");

                    let BarInfo::Memory {
                        address_type, size, ..
                    } = bar
                    else {
                        log::info!(
                            "跳过设备 {info}（位于 {} {dev_pos}）的 #{index} BAR，因为它不是内存类型",
                            node.name
                        );
                        continue;
                    };

                    if !matches!(
                        address_type,
                        MemoryBarType::Width32 | MemoryBarType::Width64
                    ) {
                        log::warn!(
                            "跳过设备 {info}（位于 {} {dev_pos}）的 #{index} BAR，因为其地址类型 {address_type:?} 不受支持",
                            node.name
                        );
                        continue;
                    }

                    let address = {
                        if let Some(size) = size.try_into().ok() {
                            bars_allocator.allocate(size)
                        } else {
                            None
                        }
                    };
                    let Some(address) = address else {
                        log::warn!(
                            "设备 {info}（位于 {} {dev_pos}）的 #{index} BAR 大小 {size:#x} 超出可分配的范围，跳过此设备，以及此 PCI 总线上的后续所有设备...",
                            node.name
                        );
                        return;
                    };

                    log::trace!(
                        "为设备 {info}（位于 {} {dev_pos}）的 #{index} BAR 分配地址：{address:#x}，大小 {size} 字节",
                        node.name
                    );

                    match address_type {
                        MemoryBarType::Width32 => pci_root.set_bar_32(dev_pos, index, address),
                        MemoryBarType::Width64 => {
                            pci_root.set_bar_64(dev_pos, index, address as u64)
                        }
                        _ => unreachable!(),
                    }
                }

                pci_root.set_command(
                    dev_pos,
                    Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER,
                );

                handle_func(PciTransport::new::<VirtIOHal, _>(&mut pci_root, dev_pos).unwrap());
            }
        }
    }
}

struct BarsBumpAllocator {
    pub current: u32,
    pub last: u32,
}

impl BarsBumpAllocator {
    /// 分配一块大小和对齐都是 `size_and_align` 的地址范围，若无法分配则返回 [`None`]。
    pub fn allocate(&mut self, size_and_align: u32) -> Option<u32> {
        let start = self
            .current
            .div_ceil(size_and_align)
            .checked_mul(size_and_align)?;
        let end = start.checked_add(size_and_align)?;

        if self.last >= end {
            self.current = end;
            Some(start)
        } else {
            None
        }
    }
}

fn select_pci_range_32(pci_ranges: impl Iterator<Item = MemoryRange>) -> Option<(u32, u32)> {
    let memories = pci_ranges.filter(|range| {
        let addr_high = range.child_bus_address_hi;
        let range_type_id = (addr_high & 0x3000000) >> 24;
        let range_end = range.child_bus_address + range.size;

        let is_memory = matches!(range_type_id, 2 | 3);
        let is_32bit = range_end < u32::MAX as _;
        let non_prefetchable = addr_high & 0x40000000 == 0;

        is_memory && non_prefetchable && is_32bit
    });

    memories.max_by_key(|range| range.size).map(|range| {
        (
            range.child_bus_address as u32,
            (range.child_bus_address + range.size) as u32,
        )
    })
}

fn handle_block_device(transport: PciTransport) {
    // FIXME: 现在这样不支持多块设备（虽然我们可能也没必要支持）
    log::warn!("初始化块设备，且将此块设备当作唯一的块设备");

    let device_box = Box::new(RwLock::new(VirtIOBlk::new(transport).unwrap()));

    OnceInited::init(&BLOCK_DEVICES, [Box::leak(device_box)]);
}

fn handle_network_device(_transport: PciTransport) {
    log::warn!("网络设备的后续步骤暂未实现");
}
