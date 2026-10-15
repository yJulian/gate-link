//! What the gate means over MQTT: topic/payload constants, how inbound
//! messages map to gate [`Command`]s, and the retained state messages that
//! mirror [`Controls`] to Home Assistant. The firmware's `mqtt_handler` and
//! `physical::gate` only move these between the broker and the gate.

use gate_core::{Controls, CoverState};

pub(crate) const NODE_ID: &str = "mqtt_gate";

pub(crate) const COVER_CONFIG_TOPIC: &str = "homeassistant/cover/mqtt_gate/config";
pub(crate) const COVER_COMMAND_TOPIC: &str = "mqtt_gate/cover/set";
pub(crate) const COVER_STATE_TOPIC: &str = "mqtt_gate/cover/state";

pub(crate) const WIND_CONFIG_TOPIC: &str = "homeassistant/binary_sensor/mqtt_gate_wind/config";
pub(crate) const WIND_STATE_TOPIC: &str = "mqtt_gate/wind/state";

/// Discovery topic of the former spare-input binary sensor (that input now
/// arms the barrier bypass) - cleared on connect so HA drops the entity.
pub(crate) const SPARE_INPUT_CONFIG_TOPIC: &str =
    "homeassistant/binary_sensor/mqtt_gate_spare_input/config";

pub(crate) const BARRIER_BYPASS_CONFIG_TOPIC: &str =
    "homeassistant/switch/mqtt_gate_barrier_bypass/config";
pub(crate) const BARRIER_BYPASS_COMMAND_TOPIC: &str = "mqtt_gate/barrier_bypass/set";
pub(crate) const BARRIER_BYPASS_STATE_TOPIC: &str = "mqtt_gate/barrier_bypass/state";

pub(crate) const BUTTON_CONFIG_TOPIC: &str = "homeassistant/button/mqtt_gate_impulse/config";
pub(crate) const BUTTON_COMMAND_TOPIC: &str = "mqtt_gate/impulse/set";
pub(crate) const RESET_WIND_LOCK_PAYLOAD: &str = "reset_anemometer";

pub(crate) const OPEN_STATE: &str = "open";
pub(crate) const CLOSED_STATE: &str = "closed";
pub(crate) const OPENING_STATE: &str = "opening";
pub(crate) const CLOSING_STATE: &str = "closing";

pub(crate) const OPEN_PAYLOAD: &str = "OPEN";
pub(crate) const CLOSE_PAYLOAD: &str = "CLOSE";
pub(crate) const STOP_PAYLOAD: &str = "STOP";

pub(crate) const ON_PAYLOAD: &str = "ON";
pub(crate) const OFF_PAYLOAD: &str = "OFF";

/// Size of the MQTT client's send and receive buffers. Every message the
/// device publishes - discovery configs included - must fit into one.
pub const MQTT_BUFFER_SIZE: usize = 512;

/// Every topic [`Command::parse`] understands; the firmware subscribes to these.
pub const COMMAND_TOPICS: [&str; 3] = [
    COVER_COMMAND_TOPIC,
    BUTTON_COMMAND_TOPIC,
    BARRIER_BYPASS_COMMAND_TOPIC,
];

/// An inbound MQTT command for the gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Open,
    Close,
    Stop,
    ResetWindLock,
    SetBarrierBypass(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandError {
    UnknownTopic,
    UnknownPayload,
}

impl Command {
    pub fn parse(topic: &str, payload: &[u8]) -> Result<Self, CommandError> {
        let payload = core::str::from_utf8(payload).unwrap_or("");
        let command = match topic {
            COVER_COMMAND_TOPIC => match payload {
                OPEN_PAYLOAD => Some(Command::Open),
                CLOSE_PAYLOAD => Some(Command::Close),
                STOP_PAYLOAD => Some(Command::Stop),
                _ => None,
            },
            BUTTON_COMMAND_TOPIC => match payload {
                RESET_WIND_LOCK_PAYLOAD => Some(Command::ResetWindLock),
                _ => None,
            },
            BARRIER_BYPASS_COMMAND_TOPIC => match payload {
                ON_PAYLOAD => Some(Command::SetBarrierBypass(true)),
                OFF_PAYLOAD => Some(Command::SetBarrierBypass(false)),
                _ => None,
            },
            _ => return Err(CommandError::UnknownTopic),
        };
        command.ok_or(CommandError::UnknownPayload)
    }

    pub fn apply(self, controls: &Controls) {
        match self {
            Command::Open => controls.open(),
            Command::Close => controls.close(),
            Command::Stop => controls.stop(),
            Command::ResetWindLock => controls.reset_wind_lock(),
            Command::SetBarrierBypass(active) => controls.set_barrier_bypass(active),
        }
    }
}

fn on_off(on: bool) -> &'static str {
    if on { ON_PAYLOAD } else { OFF_PAYLOAD }
}

/// The retained `(topic, payload)` state messages for the current state of
/// `controls`: cover position/movement, wind lock and barrier bypass.
pub fn state_messages(controls: &Controls) -> [(&'static str, &'static str); 3] {
    let cover = match controls.cover_state() {
        CoverState::Open => OPEN_STATE,
        CoverState::Opening => OPENING_STATE,
        CoverState::Closed => CLOSED_STATE,
        CoverState::Closing => CLOSING_STATE,
    };
    [
        (COVER_STATE_TOPIC, cover),
        (WIND_STATE_TOPIC, on_off(controls.wind_locked())),
        (
            BARRIER_BYPASS_STATE_TOPIC,
            on_off(controls.barrier_bypass()),
        ),
    ]
}

#[cfg(test)]
mod tests;
