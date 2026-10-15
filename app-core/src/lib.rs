//! Hardware-independent firmware logic outside of gate control: the
//! provisioning config, what the gate means over MQTT (commands, state,
//! Home Assistant discovery) and the flash records.
//!
//! Like `gate-core`, free of any ESP32/Wi-Fi dependency, so it also runs as
//! plain `cargo test` on the host (`cargo test-core`, see
//! `.cargo/config.toml`). The firmware only moves bytes between these
//! functions and the radio/flash/broker.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod discovery;
pub mod mqtt;
pub mod provisioning;
pub mod storage;
