//! # MQTT Serialization Utilities
//!
//! This module provides helper functions for reading and writing MQTT-specific data types
//! from and to byte buffers, such as variable-byte integers and length-prefixed strings.

use crate::error::{MqttError, ProtocolError};
use crate::transport;

/// Reads a variable-byte integer from the buffer, advancing the cursor.
///
/// This is a common encoding scheme in MQTT for packet lengths.
pub fn read_variable_byte_integer(
    cursor: &mut usize,
    buf: &[u8],
) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
    let mut multiplier = 1;
    let mut value = 0;
    let mut i = 0;
    loop {
        let encoded_byte = buf
            .get(*cursor + i)
            .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?;
        value += (encoded_byte & 127) as usize * multiplier;
        if (encoded_byte & 128) == 0 {
            break;
        }
        multiplier *= 128;
        i += 1;
        if i >= 4 {
            return Err(MqttError::Protocol(ProtocolError::MalformedPacket));
        }
    }
    *cursor += i + 1;
    Ok(value)
}

/// Writes a variable-byte integer to the buffer, advancing the cursor.
pub fn write_variable_byte_integer(
    cursor: &mut usize,
    buf: &mut [u8],
    mut val: usize,
) -> Result<(), MqttError<transport::ErrorPlaceHolder>> {
    loop {
        let mut encoded_byte = (val % 128) as u8;
        val /= 128;
        if val > 0 {
            encoded_byte |= 128;
        }
        *buf.get_mut(*cursor).ok_or(MqttError::BufferTooSmall)? = encoded_byte;
        *cursor += 1;
        if val == 0 {
            break;
        }
    }
    Ok(())
}

/// A simplified version of `write_variable_byte_integer` for external use that returns the byte count.
pub fn write_variable_byte_integer_len(
    buf: &mut [u8],
    mut val: usize,
) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
    let mut i = 0;
    loop {
        let mut encoded_byte = (val % 128) as u8;
        val /= 128;
        if val > 0 {
            encoded_byte |= 128;
        }
        *buf.get_mut(i).ok_or(MqttError::BufferTooSmall)? = encoded_byte;
        i += 1;
        if val == 0 {
            break;
        }
    }
    Ok(i)
}

/// Reads a UTF-8 encoded string (prefixed with a 2-byte length) from the buffer.
pub fn read_utf8_string<'a>(
    cursor: &mut usize,
    buf: &'a [u8],
) -> Result<&'a str, MqttError<transport::ErrorPlaceHolder>> {
    let len = u16::from_be_bytes(
        buf.get(*cursor..*cursor + 2)
            .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?
            .try_into()
            .unwrap(),
    ) as usize;
    *cursor += 2;
    let s = core::str::from_utf8(
        buf.get(*cursor..*cursor + len)
            .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?,
    )
    .map_err(|_| MqttError::Protocol(ProtocolError::InvalidUtf8String))?;
    *cursor += len;
    Ok(s)
}

/// Writes a UTF-8 encoded string (prefixed with a 2-byte length) to the buffer.
pub fn write_utf8_string(
    buf: &mut [u8],
    s: &str,
) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
    write_binary_data(buf, s.as_bytes())
}

/// Reads a length-prefixed (2-byte length) chunk of binary data from the buffer.
pub fn read_binary_data<'a>(
    cursor: &mut usize,
    buf: &'a [u8],
) -> Result<&'a [u8], MqttError<transport::ErrorPlaceHolder>> {
    let len = u16::from_be_bytes(
        buf.get(*cursor..*cursor + 2)
            .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?
            .try_into()
            .unwrap(),
    ) as usize;
    *cursor += 2;
    let data = buf
        .get(*cursor..*cursor + len)
        .ok_or(MqttError::Protocol(ProtocolError::MalformedPacket))?;
    *cursor += len;
    Ok(data)
}

/// Writes a length-prefixed (2-byte length) chunk of binary data to the buffer.
///
/// Used both for UTF-8 strings (client id, topic, username) and the MQTT
/// password field, which the spec defines as opaque binary data rather than text.
pub fn write_binary_data(
    buf: &mut [u8],
    data: &[u8],
) -> Result<usize, MqttError<transport::ErrorPlaceHolder>> {
    let len = data.len();
    if len > u16::MAX as usize {
        return Err(MqttError::Protocol(ProtocolError::PayloadTooLarge));
    }
    let len_bytes = (len as u16).to_be_bytes();

    let required_space = 2 + len;
    let slice = buf
        .get_mut(0..required_space)
        .ok_or(MqttError::BufferTooSmall)?;

    slice[0..2].copy_from_slice(&len_bytes);
    slice[2..].copy_from_slice(data);
    Ok(required_space)
}
