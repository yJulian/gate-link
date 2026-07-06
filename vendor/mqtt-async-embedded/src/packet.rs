//! # MQTT Packet Structures and Serialization
//!
//! This module defines the core MQTT (v3.1.1) packet types and the traits for encoding and
//! decoding them to and from a byte buffer.

use crate::error::{MqttError, ProtocolError};
use crate::transport;
use crate::util::{
    self, read_binary_data, read_utf8_string, read_variable_byte_integer, write_binary_data,
    write_utf8_string,
};

/// Represents the Quality of Service (QoS) levels for MQTT messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum QoS {
    AtMostOnce = 0,
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

/// A trait for packets that can be encoded into a byte buffer.
pub trait EncodePacket {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>>;
}

/// A trait for packets that can be decoded from a byte buffer.
pub trait DecodePacket<'a>: Sized {
    fn decode(buf: &'a [u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>>;
}

/// An enumeration of all possible MQTT control packets.
#[derive(Debug)]
pub enum MqttPacket<'a> {
    Connect(Connect<'a>),
    ConnAck(ConnAck),
    Publish(Publish<'a>),
    PubAck(PubAck),
    Subscribe(Subscribe<'a>),
    SubAck(SubAck),
    PingReq,
    PingResp,
    Disconnect,
}

/// Decodes a raw byte buffer into a specific `MqttPacket`.
pub fn decode<T>(buf: &[u8]) -> Result<Option<MqttPacket<'_>>, MqttError<T>>
where
    T: transport::TransportError,
{
    if buf.is_empty() {
        return Ok(None);
    }

    let packet_type = buf[0] >> 4;
    let packet = match packet_type {
        1 => MqttPacket::Connect(Connect::decode(buf).map_err(MqttError::cast_transport_error)?),
        2 => MqttPacket::ConnAck(ConnAck::decode(buf).map_err(MqttError::cast_transport_error)?),
        3 => MqttPacket::Publish(Publish::decode(buf).map_err(MqttError::cast_transport_error)?),
        4 => MqttPacket::PubAck(PubAck::decode(buf).map_err(MqttError::cast_transport_error)?),
        8 => {
            MqttPacket::Subscribe(Subscribe::decode(buf).map_err(MqttError::cast_transport_error)?)
        }
        9 => MqttPacket::SubAck(SubAck::decode(buf).map_err(MqttError::cast_transport_error)?),
        12 => MqttPacket::PingReq,
        13 => MqttPacket::PingResp,
        14 => MqttPacket::Disconnect,
        _ => {
            return Err(MqttError::Protocol(ProtocolError::InvalidPacketType(
                packet_type,
            )));
        }
    };

    Ok(Some(packet))
}

// --- CONNECT Packet ---
#[derive(Debug)]
pub struct Connect<'a> {
    pub clean_session: bool,
    pub keep_alive: u16,
    pub client_id: &'a str,
    pub username: Option<&'a str>,
    pub password: Option<&'a [u8]>,
}

impl<'a> Connect<'a> {
    pub fn new(client_id: &'a str, keep_alive: u16, clean_session: bool) -> Self {
        Self {
            client_id,
            keep_alive,
            clean_session,
            username: None,
            password: None,
        }
    }

    pub fn with_credentials(mut self, username: &'a str, password: &'a [u8]) -> Self {
        self.username = Some(username);
        self.password = Some(password);
        self
    }
}

impl<'a> EncodePacket for Connect<'a> {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 0;
        buf[cursor] = 0x10;
        cursor += 1;
        let remaining_len_pos = cursor;
        cursor += 4;
        let content_start = cursor;

        // MQTT v3.1.1 (OASIS): protocol name "MQTT", level 4.
        cursor += write_utf8_string(&mut buf[cursor..], "MQTT")?;
        buf[cursor] = 4;
        cursor += 1;

        let mut flags = 0u8;
        if self.clean_session {
            flags |= 0x02;
        }
        if self.username.is_some() {
            flags |= 0x80;
        }
        if self.password.is_some() {
            flags |= 0x40;
        }
        buf[cursor] = flags;
        cursor += 1;
        buf[cursor..cursor + 2].copy_from_slice(&self.keep_alive.to_be_bytes());
        cursor += 2;

        cursor += write_utf8_string(&mut buf[cursor..], self.client_id)?;
        if let Some(username) = self.username {
            cursor += write_utf8_string(&mut buf[cursor..], username)?;
        }
        if let Some(password) = self.password {
            cursor += write_binary_data(&mut buf[cursor..], password)?;
        }

        let remaining_len = cursor - content_start;
        let len_bytes =
            util::write_variable_byte_integer_len(&mut buf[remaining_len_pos..], remaining_len)?;
        let header_len = 1 + len_bytes;
        buf.copy_within(content_start..cursor, header_len);
        Ok(header_len + remaining_len)
    }
}
impl<'a> DecodePacket<'a> for Connect<'a> {
    fn decode(buf: &'a [u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 2;
        cursor += 6;
        let connect_flags = buf[cursor];
        let clean_session = (connect_flags & 0x02) != 0;
        cursor += 1;
        let keep_alive = u16::from_be_bytes([buf[cursor], buf[cursor + 1]]);
        cursor += 2;
        let client_id = read_utf8_string(&mut cursor, buf)?;
        let username = if connect_flags & 0x80 != 0 {
            Some(read_utf8_string(&mut cursor, buf)?)
        } else {
            None
        };
        let password = if connect_flags & 0x40 != 0 {
            Some(read_binary_data(&mut cursor, buf)?)
        } else {
            None
        };
        Ok(Self {
            clean_session,
            keep_alive,
            client_id,
            username,
            password,
        })
    }
}

// --- CONNACK Packet ---
#[derive(Debug)]
pub struct ConnAck {
    pub session_present: bool,
    pub reason_code: u8,
}
impl DecodePacket<'_> for ConnAck {
    fn decode(buf: &[u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 2;
        let session_present = (buf[cursor] & 0x01) != 0;
        cursor += 1;
        let reason_code = buf[cursor];
        Ok(Self {
            session_present,
            reason_code,
        })
    }
}

// --- PUBLISH Packet ---
#[derive(Debug)]
pub struct Publish<'a> {
    pub topic: &'a str,
    pub qos: QoS,
    pub retain: bool,
    pub payload: &'a [u8],
    pub packet_id: Option<u16>,
}
impl<'a> DecodePacket<'a> for Publish<'a> {
    fn decode(buf: &'a [u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        let qos = match (buf[0] >> 1) & 0x03 {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            2 => QoS::ExactlyOnce,
            _ => return Err(MqttError::Protocol(ProtocolError::MalformedPacket)),
        };
        let retain = (buf[0] & 0x01) != 0;
        let mut cursor = 1;
        let _remaining_len = read_variable_byte_integer(&mut cursor, buf)?;
        let topic = read_utf8_string(&mut cursor, buf)?;
        let packet_id = if qos != QoS::AtMostOnce {
            let id = u16::from_be_bytes(
                buf.get(cursor..cursor + 2)
                    .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?
                    .try_into()
                    .unwrap(),
            );
            cursor += 2;
            Some(id)
        } else {
            None
        };
        let payload = buf
            .get(cursor..)
            .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?;
        Ok(Publish {
            topic,
            qos,
            retain,
            payload,
            packet_id,
        })
    }
}
impl<'a> EncodePacket for Publish<'a> {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 0;
        let header_byte_pos = cursor;
        cursor += 1;
        let remaining_len_pos = cursor;
        cursor += 4;
        let content_start = cursor;

        cursor += write_utf8_string(&mut buf[cursor..], self.topic)?;
        if self.qos != QoS::AtMostOnce {
            let packet_id = self
                .packet_id
                .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?;
            buf.get_mut(cursor..cursor + 2)
                .ok_or(MqttError::BufferTooSmall)?
                .copy_from_slice(&packet_id.to_be_bytes());
            cursor += 2;
        }

        let payload_space = buf
            .get_mut(cursor..cursor + self.payload.len())
            .ok_or(MqttError::BufferTooSmall)?;
        payload_space.copy_from_slice(self.payload);
        cursor += self.payload.len();

        let remaining_len = cursor - content_start;
        let len_bytes =
            util::write_variable_byte_integer_len(&mut buf[remaining_len_pos..], remaining_len)?;
        let header_len = 1 + len_bytes;
        buf.copy_within(content_start..cursor, header_len);
        buf[header_byte_pos] = 0x30 | ((self.qos as u8) << 1) | (self.retain as u8);
        Ok(header_len + remaining_len)
    }
}

// --- PUBACK Packet ---
#[derive(Debug)]
pub struct PubAck {
    pub packet_id: u16,
}
impl DecodePacket<'_> for PubAck {
    fn decode(buf: &[u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        let packet_id = u16::from_be_bytes(
            buf.get(2..4)
                .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?
                .try_into()
                .unwrap(),
        );
        Ok(PubAck { packet_id })
    }
}
impl EncodePacket for PubAck {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        buf.get_mut(0..4)
            .ok_or(MqttError::BufferTooSmall)?
            .copy_from_slice(&[
                0x40,
                0x02,
                (self.packet_id >> 8) as u8,
                (self.packet_id & 0xFF) as u8,
            ]);
        Ok(4)
    }
}

// --- SUBSCRIBE Packet ---
#[derive(Debug)]
pub struct Subscribe<'a> {
    pub packet_id: u16,
    pub topics: heapless::Vec<(&'a str, QoS), 8>,
}
impl<'a> DecodePacket<'a> for Subscribe<'a> {
    fn decode(_buf: &'a [u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        Ok(Subscribe {
            packet_id: 0,
            topics: heapless::Vec::new(),
        })
    }
}
impl<'a> EncodePacket for Subscribe<'a> {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 0;
        buf[cursor] = 0x82;
        cursor += 1;
        let remaining_len_pos = cursor;
        cursor += 4;
        let content_start = cursor;

        buf[cursor..cursor + 2].copy_from_slice(&self.packet_id.to_be_bytes());
        cursor += 2;
        for (topic, qos) in &self.topics {
            cursor += write_utf8_string(&mut buf[cursor..], topic)?;
            buf[cursor] = *qos as u8;
            cursor += 1;
        }

        let remaining_len = cursor - content_start;
        let len_bytes =
            util::write_variable_byte_integer_len(&mut buf[remaining_len_pos..], remaining_len)?;
        let header_len = 1 + len_bytes;
        buf.copy_within(content_start..cursor, header_len);
        Ok(header_len + remaining_len)
    }
}

// --- SUBACK Packet ---
#[derive(Debug)]
pub struct SubAck {
    pub packet_id: u16,
    pub reason_codes: heapless::Vec<u8, 8>,
}
impl DecodePacket<'_> for SubAck {
    fn decode(buf: &[u8]) -> Result<Self, MqttError<transport::ErrorPlaceHolder>> {
        let mut cursor = 2;
        let _remaining_len = read_variable_byte_integer(&mut cursor, buf)?;
        let packet_id = u16::from_be_bytes(
            buf.get(cursor..cursor + 2)
                .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?
                .try_into()
                .unwrap(),
        );
        cursor += 2;
        let mut reason_codes = heapless::Vec::new();
        for &b in buf.get(cursor..).unwrap_or(&[]) {
            reason_codes
                .push(b)
                .map_err(|_| MqttError::BufferTooSmall)?;
        }
        Ok(SubAck {
            packet_id,
            reason_codes,
        })
    }
}

// --- PINGREQ Packet ---
#[derive(Debug)]
pub struct PingReq;
impl EncodePacket for PingReq {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        if buf.len() < 2 {
            return Err(MqttError::BufferTooSmall);
        }
        buf[0] = 0xC0;
        buf[1] = 0x00;
        Ok(2)
    }
}

// --- PINGRESP Packet ---
#[derive(Debug)]
pub struct PingResp;

// --- DISCONNECT Packet ---
#[derive(Debug)]
pub struct Disconnect;
impl EncodePacket for Disconnect {
    fn encode(&self, buf: &mut [u8]) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
        if buf.len() < 2 {
            return Err(MqttError::BufferTooSmall);
        }
        buf[0] = 0xE0;
        buf[1] = 0x00;
        Ok(2)
    }
}
