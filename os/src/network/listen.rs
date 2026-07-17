use core::{
    hash::Hash,
    net::{Ipv4Addr, Ipv6Addr},
};

use hashbrown::{Equivalent, HashMap, HashSet, hash_map};

use crate::network::is_addr_unspecified;

pub struct ListeningTable {
    table: HashSet<ListeningTableEntry>,
    specified_counts: HashMap<u16, u32>,
}

impl ListeningTable {
    pub fn new() -> Self {
        Self {
            table: HashSet::new(),
            specified_counts: HashMap::new(),
        }
    }

    pub fn find(&self, address: Ipv6Addr, port: u16) -> Option<&ListeningTableEntry> {
        self.table.get(&EntryAddress(address, port))
    }

    pub fn insert_unchecked(&mut self, entry: ListeningTableEntry) {
        if !is_addr_unspecified(entry.address) {
            *self.specified_counts.entry(entry.port).or_default() += 1;
        }

        debug_assert!(!self.table.contains(&entry));
        unsafe { self.table.insert_unique_unchecked(entry) };
    }

    pub fn remove(&mut self, address: Ipv6Addr, port: u16) -> Result<(), ()> {
        let success = self.table.remove(&EntryAddress(address, port));

        if !success {
            return Err(());
        }

        if !is_addr_unspecified(address) {
            let hash_map::Entry::Occupied(mut entry) = self.specified_counts.entry(port) else {
                unreachable!();
            };

            let val = entry.get_mut();

            if *val != 1 {
                *val -= 1;
            } else {
                entry.remove();
            }
        }

        Ok(())
    }

    pub fn try_insert(&mut self, entry: ListeningTableEntry) -> Result<(), ListeningTableEntry> {
        if self.table.contains(&entry) {
            return Err(entry);
        }

        if is_addr_unspecified(entry.address) && self.specified_counts.contains_key(&entry.port) {
            return Err(entry);
        }

        if self
            .table
            .contains(&EntryAddress(Ipv6Addr::UNSPECIFIED, entry.port))
            || self.table.contains(&EntryAddress(
                Ipv4Addr::UNSPECIFIED.to_ipv6_mapped(),
                entry.port,
            ))
        {
            return Err(entry);
        }

        self.insert_unchecked(entry);
        Ok(())
    }
}

pub struct ListeningTableEntry {
    address: Ipv6Addr,
    port: u16,
    // pub full_connects: VecDeque<smoltcp::iface::SocketHandle>
}

impl Hash for ListeningTableEntry {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
        self.port.hash(state);
    }
}

impl PartialEq for ListeningTableEntry {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address && self.port == other.port
    }
}

impl Eq for ListeningTableEntry {}

impl ListeningTableEntry {
    pub fn new(address: Ipv6Addr, port: u16) -> Self {
        Self { address, port }
    }

    pub fn addr(&self) -> (Ipv6Addr, u16) {
        (self.address, self.port)
    }
}

struct EntryAddress(Ipv6Addr, u16);

impl Hash for EntryAddress {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
        self.1.hash(state);
    }
}

impl Equivalent<ListeningTableEntry> for EntryAddress {
    fn equivalent(&self, key: &ListeningTableEntry) -> bool {
        self.0 == key.address && self.1 == key.port
    }
}
