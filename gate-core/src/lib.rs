//! Hardware-independent gate control logic.
//!
//! Everything that decides *when which relay switches* lives here, generic
//! over `embedded_hal::digital::OutputPin` and free of any ESP32/Wi-Fi/MQTT
//! dependency - so it builds for the firmware target and also runs as plain
//! `cargo test` on the host (see `.cargo/config.toml` alias `test-core`),
//! with embassy-time's mock driver standing in for the hardware clock.
//!
//! The firmware (`src/physical/gate.rs`) only adds the glue: the static
//! [`Controls`] instance, MQTT publishing and flash persistence.

#![cfg_attr(not(test), no_std)]

mod debounce;
mod gate;
mod motor;
mod warning_light;

#[cfg(test)]
mod sim;

pub use debounce::Debouncer;
pub use gate::{Controls, CoverState, Gate, GateCommand, Snapshot};
pub use motor::{Direction, LeafTiming, Motor};
pub use warning_light::warning_light;
