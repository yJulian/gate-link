//! # The Asynchronous MQTT Client
//!
//! This module contains the primary `MqttClient` struct, which manages the state,
//! connection, and communication with an MQTT broker.

use crate::error::{MqttError, ProtocolError};
use crate::packet::{
    self, Connect, DecodePacket, EncodePacket, MqttPacket, PingReq, PubAck, Publish, QoS,
};
use crate::transport::{self, MqttTransport};
use crate::util;
use embassy_time::{Duration, Instant};

/// Configuration options for the `MqttClient`.
pub struct MqttOptions<'a> {
    client_id: &'a str,
    #[allow(
        dead_code,
        reason = "kept for API parity / future use by callers inspecting options"
    )]
    broker_addr: &'a str,
    #[allow(
        dead_code,
        reason = "kept for API parity / future use by callers inspecting options"
    )]
    broker_port: u16,
    keep_alive: Duration,
    username: Option<&'a str>,
    password: Option<&'a [u8]>,
}

impl<'a> MqttOptions<'a> {
    pub fn new(client_id: &'a str, broker_addr: &'a str, broker_port: u16) -> Self {
        Self {
            client_id,
            broker_addr,
            broker_port,
            keep_alive: Duration::from_secs(60),
            username: None,
            password: None,
        }
    }

    pub fn with_keep_alive(mut self, keep_alive: Duration) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    /// Sets the username/password sent in the CONNECT packet (MQTT 3.1.1 §3.1.3).
    pub fn with_credentials(mut self, username: &'a str, password: &'a [u8]) -> Self {
        self.username = Some(username);
        self.password = Some(password);
        self
    }
}

/// Represents the current connection state of the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

/// The asynchronous MQTT client.
pub struct MqttClient<'a, T, const MAX_TOPICS: usize, const BUF_SIZE: usize>
where
    T: MqttTransport,
{
    transport: T,
    options: MqttOptions<'a>,
    tx_buffer: [u8; BUF_SIZE],
    rx_buffer: [u8; BUF_SIZE],
    /// How many bytes of `rx_buffer` hold received, not yet dropped data.
    rx_len: usize,
    /// Length of the packet at the front of `rx_buffer` that was already
    /// handed out; dropped before the next read.
    rx_consumed: usize,
    state: ConnectionState,
    last_tx_time: Instant,
    next_packet_id: u16,
}

impl<'a, T, const MAX_TOPICS: usize, const BUF_SIZE: usize> MqttClient<'a, T, MAX_TOPICS, BUF_SIZE>
where
    T: MqttTransport,
{
    pub fn new(transport: T, options: MqttOptions<'a>) -> Self {
        Self {
            transport,
            options,
            tx_buffer: [0; BUF_SIZE],
            rx_buffer: [0; BUF_SIZE],
            rx_len: 0,
            rx_consumed: 0,
            state: ConnectionState::Disconnected,
            last_tx_time: Instant::now(),
            next_packet_id: 1,
        }
    }

    /// Attempts to connect to the MQTT broker.
    pub async fn connect(&mut self) -> Result<(), MqttError<T::Error>>
    where
        T::Error: transport::TransportError,
    {
        self.state = ConnectionState::Connecting;
        let mut connect_packet = Connect::new(
            self.options.client_id,
            self.options.keep_alive.as_secs() as u16,
            true,
        );
        if let (Some(username), Some(password)) = (self.options.username, self.options.password) {
            connect_packet = connect_packet.with_credentials(username, password);
        }
        let len = connect_packet
            .encode(&mut self.tx_buffer)
            .map_err(MqttError::cast_transport_error)?;
        self.transport.send(&self.tx_buffer[..len]).await?;
        self.rx_len = 0;
        self.rx_consumed = 0;
        let len = self
            .next_packet()
            .await?
            .ok_or(MqttError::Protocol(ProtocolError::InvalidResponse))?;
        self.rx_consumed = len;
        let packet = packet::decode::<T::Error>(&self.rx_buffer[..len])?
            .ok_or(MqttError::Protocol(ProtocolError::InvalidResponse))?;
        if let MqttPacket::ConnAck(connack) = packet {
            if connack.reason_code == 0 {
                self.state = ConnectionState::Connected;
                self.last_tx_time = Instant::now();
                Ok(())
            } else {
                self.state = ConnectionState::Disconnected;
                Err(MqttError::ConnectionRefused(connack.reason_code.into()))
            }
        } else {
            self.state = ConnectionState::Disconnected;
            Err(MqttError::Protocol(ProtocolError::InvalidResponse))
        }
    }

    /// Publishes a message to a topic.
    pub async fn publish<'p>(
        &mut self,
        topic: &'p str,
        payload: &'p [u8],
        qos: QoS,
        retain: bool,
    ) -> Result<(), MqttError<T::Error>>
    where
        T::Error: transport::TransportError,
    {
        let packet_id = if qos == QoS::AtMostOnce {
            None
        } else {
            Some(self.get_next_packet_id())
        };
        let publish_packet = Publish {
            topic,
            qos,
            retain,
            payload,
            packet_id,
        };
        self._send_packet(publish_packet).await
    }

    /// Subscribes to a topic.
    pub async fn subscribe(&mut self, topic: &str, qos: QoS) -> Result<(), MqttError<T::Error>>
    where
        T::Error: transport::TransportError,
    {
        let mut topics = heapless::Vec::new();
        topics
            .push((topic, qos))
            .map_err(|_| MqttError::BufferTooSmall)?;
        let subscribe_packet = packet::Subscribe {
            packet_id: self.get_next_packet_id(),
            topics,
        };
        self._send_packet(subscribe_packet).await
    }

    /// Sends a pre-constructed packet over the transport.
    async fn _send_packet<P>(&mut self, packet: P) -> Result<(), MqttError<T::Error>>
    where
        P: EncodePacket,
        T::Error: transport::TransportError,
    {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }
        let len = packet
            .encode(&mut self.tx_buffer)
            .map_err(MqttError::cast_transport_error)?;
        self.transport.send(&self.tx_buffer[..len]).await?;
        self.last_tx_time = Instant::now();
        Ok(())
    }

    /// Polls the connection for incoming packets and handles keep-alives.
    /// Handles at most one packet per call; returns `Ok(None)` for anything
    /// that isn't a PUBLISH (acks, ping responses, unknown packet types).
    /// Incoming QoS 1 PUBLISHes are acknowledged with a PUBACK.
    ///
    /// Cancel-safe: dropping the future never loses received bytes.
    ///
    /// The returned `MqttEvent` contains references to the client's internal receive
    /// buffer. These references are only valid until the next call to `poll`.
    pub async fn poll<'p>(&'p mut self) -> Result<Option<MqttEvent<'p>>, MqttError<T::Error>>
    where
        T::Error: transport::TransportError,
    {
        if self.state != ConnectionState::Connected {
            return Err(MqttError::NotConnected);
        }

        if self.last_tx_time.elapsed() >= self.options.keep_alive {
            self._send_packet(PingReq).await?;
        }

        let Some(len) = self.next_packet().await? else {
            return Ok(None);
        };
        const PUBLISH: u8 = 3;
        if self.rx_buffer[0] >> 4 != PUBLISH {
            self.rx_consumed = len;
            return Ok(None);
        }
        let publish =
            Publish::decode(&self.rx_buffer[..len]).map_err(MqttError::cast_transport_error)?;
        if let (QoS::AtLeastOnce, Some(packet_id)) = (publish.qos, publish.packet_id) {
            // Only marked consumed after the ack went out: if this future is
            // dropped mid-send, the next poll delivers the packet again.
            self._send_packet(PubAck { packet_id }).await?;
        }
        self.rx_consumed = len;
        let publish =
            Publish::decode(&self.rx_buffer[..len]).map_err(MqttError::cast_transport_error)?;
        Ok(Some(MqttEvent::Publish(publish)))
    }

    /// Drops the packet handed out last, then reads until a complete packet
    /// is at the front of `rx_buffer` and returns its length. `None` if the
    /// transport returned no data.
    async fn next_packet(&mut self) -> Result<Option<usize>, MqttError<T::Error>>
    where
        T::Error: transport::TransportError,
    {
        if self.rx_consumed > 0 {
            self.rx_buffer.copy_within(self.rx_consumed..self.rx_len, 0);
            self.rx_len -= self.rx_consumed;
            self.rx_consumed = 0;
        }
        loop {
            if let Some(len) = util::complete_packet_len(&self.rx_buffer[..self.rx_len])
                .map_err(MqttError::cast_transport_error)?
            {
                return Ok(Some(len));
            }
            if self.rx_len == BUF_SIZE {
                return Err(MqttError::BufferTooSmall);
            }
            let n = self
                .transport
                .recv(&mut self.rx_buffer[self.rx_len..])
                .await?;
            if n == 0 {
                return Ok(None);
            }
            self.rx_len += n;
        }
    }

    fn get_next_packet_id(&mut self) -> u16 {
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if self.next_packet_id == 0 {
            self.next_packet_id = 1;
        }
        self.next_packet_id
    }
}

/// Represents an event received from the MQTT broker.
/// The lifetime `'p` indicates that the event borrows data from the client's
/// buffer and is only valid for the duration of the `poll` call.
#[derive(Debug)]
pub enum MqttEvent<'p> {
    Publish(Publish<'p>),
}
