//! Channel multiplexer configuration.

use std::time::Duration;

use crate::rch;

use super::msg::MAX_MSG_LENGTH;

/// Behavior when ports are exhausted and a connect is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PortsExhausted {
    /// Immediately fail connect request.
    Fail,
    /// Wait for a port to become available with an optional timeout.
    Wait(Option<Duration>),
}

/// Channel multiplexer configuration.
///
/// In most cases the default configuration ([Cfg::default]) is recommended, since it
/// provides a good balance between throughput, memory usage and latency.
///
/// In case of unsatisfactory performance (low throughput) your first step should be
/// to increase the [receive buffer size](Self::receive_buffer).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Cfg {
    /// Time after which connection is closed when no data is.
    ///
    /// Pings are send automatically when this is enabled and no data is transmitted.
    /// By default this is 60 seconds.
    pub connection_timeout: Option<Duration>,
    /// Maximum number of open ports.
    ///
    /// This must not exceed 2^31 = 2147483648.
    /// By default this is 16384.
    pub max_ports: u32,
    /// Default behavior when ports are exhausted and a connect is requested.
    ///
    /// This can be overridden on a per-request basis.
    /// By default this is wait with a timeout of 60 seconds.
    pub ports_exhausted: PortsExhausted,
    /// Maximum size of received data per message in bytes.
    ///
    /// [Receiver::recv_chunk](super::Receiver::recv_chunk) is not affected by this limit.
    ///
    /// [Remote channels](crate::rch) will spawn a serialization and deserialization thread
    /// to transmit and receive data in chunks if this limit is reached.
    /// Thus, this does not limit the maximum serialized data size for remote channels
    /// but will incur a small performance cost for inter-thread communication when exceeded.
    ///
    /// This can be configured on a per-receiver basis.
    /// By default this is 512 kB.
    pub max_data_size: usize,
    /// Maximum port requests received per message.
    ///
    /// For [remote channels](crate::rch) this configures how many more ports than expected
    /// (from the data type) can be received per message.
    /// This is useful for compatibility when the receiver has an older version of a struct
    /// type with less fields containing ports.
    ///
    /// This can be configured on a per-receiver basis.
    /// By default this is 128.
    pub max_received_ports: usize,
    /// Size of a chunk of data in bytes.
    ///
    /// By default this is 16 kB.
    /// This must be at least 4 bytes.
    /// This must not exceed 2^32 - 16 = 4294967279.
    pub chunk_size: u32,
    /// Maximum size of a single item transmitted in bytes.
    ///
    /// By default this is 16 MB (defined in [rch::DEFAULT_MAX_ITEM_SIZE]).
    /// An error was threw if a request size exceeds this value.
    pub max_item_size: usize,
    /// Size of receive buffer of each port in bytes.
    ///
    /// This controls the maximum amount of in-flight data per port, that is data on the transport
    /// plus received but yet unprocessed data.
    ///
    /// Increase this value if the throughput (bytes per second) is significantly
    /// lower than you would expect from your underlying transport connection.
    ///
    /// By default this is 512 kB.
    /// This must be at least 4 bytes.
    pub receive_buffer: u32,
    /// Length of global send queue.
    /// Each element holds a chunk.
    ///
    /// This limits the number of chunks sendable by using
    /// [Sender::try_send](super::Sender::try_send).
    /// It will not affect [remote channels](crate::rch).
    ///
    /// By default this is 16.
    /// This must not be zero.
    pub shared_send_queue: usize,
    /// Length of transport send queue.
    /// Each element holds a chunk.
    ///
    /// Raising this may improve performance but might incur a slight increase in latency.
    /// For minimum latency this should be set to 1.
    ///
    /// By default this is 16.
    /// This must not be zero.
    pub transport_send_queue: usize,
    /// Length of transport receive queue.
    /// Each element holds a chunk.
    ///
    /// Raising this may improve performance but might incur a slight increase in latency.
    /// For minimum latency this should be set to 1.
    ///
    /// By default this is 16.
    /// This must not be zero.
    pub transport_receive_queue: usize,
    /// Maximum number of outstanding connection requests.
    ///
    /// By default this is 128.
    /// This must not be zero.
    pub connect_queue: u16,
    /// Time to wait when no data is available for sending before flushing the send buffer
    /// of the connection.
    ///
    /// By default this is 20 milliseconds.
    pub flush_delay: Duration,
    #[doc(hidden)]
    pub _non_exhaustive: (),
}

impl Default for Cfg {
    /// The default configuration provides a balance between throughput,
    /// memory usage and latency.
    fn default() -> Self {
        Self {
            connection_timeout: Some(Duration::from_secs(60)),
            max_ports: 16_384,
            ports_exhausted: PortsExhausted::Wait(Some(Duration::from_secs(60))),
            max_data_size: 524_288,
            max_received_ports: 128,
            chunk_size: 16_384,
            max_item_size: rch::DEFAULT_MAX_ITEM_SIZE,
            receive_buffer: 524_288,
            shared_send_queue: 16,
            transport_send_queue: 16,
            transport_receive_queue: 16,
            connect_queue: 128,
            flush_delay: Duration::from_millis(20),
            _non_exhaustive: (),
        }
    }
}

impl Cfg {
    /// Checks the configuration.
    ///
    /// # Panics
    /// Panics if the configuration is invalid.
    pub(crate) fn check(&self) {
        if self.max_ports > 2u32.pow(31) {
            panic!("maximum ports must not exceed 2^31");
        }

        if self.chunk_size < 4 {
            panic!("chunk size must be at least 4");
        }

        if self.receive_buffer < 4 {
            panic!("receive buffer must be at least 4 bytes");
        }

        if self.shared_send_queue == 0 {
            panic!("shared send queue length must not be zero");
        }

        if self.transport_send_queue == 0 {
            panic!("transport send queue length must not be zero");
        }

        if self.transport_receive_queue == 0 {
            panic!("transport receive queue length must not be zero");
        }

        if self.connect_queue == 0 {
            panic!("connect queue length must not be zero");
        }
    }

    /// Returns the maximum size of a frame that can be received by a
    /// channel multiplexer using this configuration.
    ///
    /// # Panics
    /// Panics if the configuration is invalid.
    pub fn max_frame_length(&self) -> u32 {
        (MAX_MSG_LENGTH as u32).checked_add(self.chunk_size).expect("maximum frame size exceeds u32::MAX")
    }

    /// Configuration that is balanced between memory usage, latency and throughput.
    pub fn balanced() -> Self {
        Self::default()
    }

    /// Configuration that is optimized for low memory usage and low latency
    /// but may be throughput-limited.
    pub fn compact() -> Self {
        Self {
            shared_send_queue: 1,
            transport_receive_queue: 1,
            transport_send_queue: 1,
            receive_buffer: 16_384,
            chunk_size: 4096,
            ..Default::default()
        }
    }

    /// Configuration that is throughput-optimized but may use more memory per
    /// channel and may have higher latency.
    pub fn throughput() -> Self {
        Self {
            shared_send_queue: 64,
            transport_receive_queue: 64,
            transport_send_queue: 64,
            receive_buffer: 1_048_576,
            chunk_size: 32_768,
            ..Default::default()
        }
    }
}
