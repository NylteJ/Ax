/// Maximum size of transport-layer payload buffered for single-packet ingress.
///
/// This must be at least as large as the device MTU minus IP header overhead.
/// 1500 bytes covers standard Ethernet MTU (1500) minus typical IP header (20–60 bytes).
pub const INGRESS_PACKET_BUFFER_SIZE: usize = 1500;

/// A parsed incoming network packet, owning its transport-layer data.
///
/// Produced by [`Interface::poll_ingress_read`] and consumed by
/// [`Interface::poll_ingress_deliver`]. The caller inspects [`ip_repr`](Self::ip_repr)
/// to determine which socket the packet should be delivered to.
///
/// The transport-layer segment (TCP, UDP, ICMP, etc.) is copied into an owned
/// buffer during parsing so it survives beyond the device's RX token lifetime.
/// This is a necessary copy: in the split ingress model (read → lookup socket →
/// deliver), the packet data must outlive the `rx_token.consume()` closure.
///
/// [`Interface::poll_ingress_read`]: crate::iface::Interface::poll_ingress_read
/// [`Interface::poll_ingress_deliver`]: crate::iface::Interface::poll_ingress_deliver
pub struct IngressPacket {
    /// The parsed IP-layer representation (source/destination addresses,
    /// protocol type, payload length).
    pub ip_repr: crate::wire::IpRepr,

    /// Packet metadata from the device receive token (timestamp, etc.).
    pub meta: crate::phy::PacketMeta,

    /// The source hardware address of the received frame.
    /// Used to update the neighbor cache on receipt of unicast packets.
    #[cfg(any(feature = "medium-ethernet", feature = "medium-ieee802154"))]
    pub src_hardware_addr: crate::wire::HardwareAddress,

    /// Transport-layer source port (TCP/UDP).
    /// Set to 0 for protocols without ports (ICMP, Raw IP, etc.).
    src_port: u16,

    /// Transport-layer destination port (TCP/UDP).
    /// Set to 0 for protocols without ports (ICMP, Raw IP, etc.).
    dst_port: u16,

    /// Buffer holding the transport-layer segment as raw bytes.
    /// Parsed into the appropriate repr during delivery.
    buffer: [u8; INGRESS_PACKET_BUFFER_SIZE],

    /// Number of valid bytes in `buffer`.
    buffer_len: usize,
}

impl IngressPacket {
    /// Create a new `IngressPacket`.
    ///
    /// The internal state is not meaningful until [`Interface::poll_ingress_read`]
    /// successfully processes a packet into this buffer.
    ///
    /// [`Interface::poll_ingress_read`]: crate::iface::Interface::poll_ingress_read
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            // The ip_repr is always overwritten by poll_ingress_read before being
            // read. We use a dummy IPv4 repr here; the actual value doesn't matter.
            #[cfg(feature = "proto-ipv4")]
            ip_repr: {
                crate::wire::IpRepr::Ipv4(crate::wire::Ipv4Repr {
                    src_addr: crate::wire::Ipv4Address::UNSPECIFIED,
                    dst_addr: crate::wire::Ipv4Address::UNSPECIFIED,
                    next_header: crate::wire::IpProtocol::Tcp,
                    payload_len: 0,
                    hop_limit: 0,
                })
            },
            #[cfg(all(feature = "proto-ipv6", not(feature = "proto-ipv4")))]
            ip_repr: {
                crate::wire::IpRepr::Ipv6(crate::wire::Ipv6Repr {
                    src_addr: crate::wire::Ipv6Address::UNSPECIFIED,
                    dst_addr: crate::wire::Ipv6Address::UNSPECIFIED,
                    next_header: crate::wire::IpProtocol::Tcp,
                    payload_len: 0,
                    hop_limit: 0,
                })
            },
            meta: crate::phy::PacketMeta::default(),
            #[cfg(any(feature = "medium-ethernet", feature = "medium-ieee802154"))]
            src_hardware_addr: crate::wire::HardwareAddress::Ethernet(
                crate::wire::EthernetAddress([0; 6]),
            ),
            src_port: 0,
            dst_port: 0,
            buffer: [0u8; INGRESS_PACKET_BUFFER_SIZE],
            buffer_len: 0,
        }
    }

    /// Transport-layer source port.
    ///
    /// For TCP and UDP packets, this is the source port from the transport header.
    /// For other protocols (ICMP, Raw IP), this is 0.
    pub fn src_port(&self) -> u16 {
        self.src_port
    }

    /// Transport-layer destination port.
    ///
    /// For TCP and UDP packets, this is the destination port from the transport header.
    /// For other protocols (ICMP, Raw IP), this is 0.
    pub fn dst_port(&self) -> u16 {
        self.dst_port
    }

    /// Get the transport-layer payload as a byte slice.
    pub fn transport_data(&self) -> &[u8] {
        &self.buffer[..self.buffer_len]
    }

    /// Set the transport-layer payload data and extract ports.
    ///
    /// For TCP/UDP, the first 4 bytes are parsed as src_port and dst_port (big-endian u16).
    /// For other protocols, ports remain 0.
    ///
    /// # Panics
    ///
    /// Panics if `data.len()` exceeds `INGRESS_PACKET_BUFFER_SIZE`.
    pub(crate) fn set_transport_data(&mut self, data: &[u8]) {
        assert!(data.len() <= INGRESS_PACKET_BUFFER_SIZE);
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer_len = data.len();
    }

    /// Set the transport-layer payload data and extract ports from the header.
    ///
    /// For TCP and UDP, the first 4 bytes of the transport header are `src_port` and
    /// `dst_port` (big-endian u16). This method parses them. For non-port protocols,
    /// `set_ports(false)` can be called after to leave ports at 0.
    ///
    /// # Panics
    ///
    /// Panics if `data.len()` exceeds `INGRESS_PACKET_BUFFER_SIZE`.
    /// If `has_ports` is true, panics if `data.len() < 4`.
    pub(crate) fn set_transport_data_with_ports(&mut self, data: &[u8], has_ports: bool) {
        assert!(data.len() <= INGRESS_PACKET_BUFFER_SIZE);
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer_len = data.len();

        if has_ports {
            assert!(data.len() >= 4);
            self.src_port = u16::from_be_bytes([self.buffer[0], self.buffer[1]]);
            self.dst_port = u16::from_be_bytes([self.buffer[2], self.buffer[3]]);
        }
    }

    /// Set the transport-layer payload data without bounds checking.
    ///
    /// # Safety
    ///
    /// `data.len()` must not exceed `INGRESS_PACKET_BUFFER_SIZE`.
    pub(crate) unsafe fn set_transport_data_unchecked(&mut self, data: &[u8]) {
        debug_assert!(data.len() <= INGRESS_PACKET_BUFFER_SIZE);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.buffer.as_mut_ptr(),
                data.len(),
            );
        }
        self.buffer_len = data.len();
    }

    /// Parse source and destination ports from the transport header.
    ///
    /// Reads the first 4 bytes of the buffer as two big-endian u16 values.
    /// This is valid for TCP and UDP headers. Call after `set_transport_data`.
    ///
    /// # Panics
    ///
    /// Panics if `buffer_len < 4`.
    pub(crate) fn parse_ports(&mut self) {
        assert!(self.buffer_len >= 4);
        self.src_port = u16::from_be_bytes([self.buffer[0], self.buffer[1]]);
        self.dst_port = u16::from_be_bytes([self.buffer[2], self.buffer[3]]);
    }

    /// Get the transport data payload from this packet.
    ///
    /// Returns a reference to the internal buffer containing the transport-layer
    /// header and payload, up to `buffer_len` bytes. The data starts at the
    /// beginning of the transport header (e.g., the first byte of the TCP/UDP header).
    pub(crate) fn transport_data_mut(&mut self) -> &mut [u8] {
        &mut self.buffer[..self.buffer_len]
    }
}
