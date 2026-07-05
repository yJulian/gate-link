//! # MQTT Transport Abstraction
//!
//! This module defines the `MqttTransport` trait, which abstracts the underlying
//! communication channel (like TCP, UART, etc.), allowing the MQTT client to be
//! hardware and network-stack agnostic.
//!
//! `mqtt_gate` provides its own `MqttTransport` impl over its `embassy-net` TCP
//! socket (see `src/mqtt_client.rs` in the main crate) rather than a built-in one
//! here, to avoid pinning this crate to a specific `embassy-net` version.

/// A placeholder error type used in contexts where the actual transport error is not known,
/// such as in the `EncodePacket` trait.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ErrorPlaceHolder;

/// A trait representing a transport for MQTT packets.
///
/// This trait abstracts over any reliable, ordered, stream-based communication channel.
#[allow(
    async_fn_in_trait,
    reason = "single-executor no_std crate; Send bounds on the returned futures aren't needed"
)]
pub trait MqttTransport {
    /// The error type returned by the transport.
    type Error: core::fmt::Debug;

    /// Sends a buffer of data over the transport.
    async fn send(&mut self, buf: &[u8]) -> Result<(), Self::Error>;

    /// Receives data from the transport into a buffer.
    ///
    /// Returns the number of bytes read.
    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

// Allow the placeholder to be treated as a transport error for generic contexts.
impl TransportError for ErrorPlaceHolder {}

/// A marker trait for transport-related errors.
pub trait TransportError: core::fmt::Debug {}
