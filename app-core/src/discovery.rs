//! Home Assistant MQTT Discovery configs, so the gate, its wind-lock sensor,
//! the wind-lock-reset button and the light barrier bypass switch show up as
//! entities without manual YAML.
//!
//! <https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery>

use alloc::vec::Vec;

use serde::Serialize;

use crate::mqtt::*;

#[derive(Serialize, Clone, Copy)]
struct Device {
    identifiers: [&'static str; 1],
    name: &'static str,
    manufacturer: &'static str,
    model: &'static str,
    sw_version: &'static str,
}

#[derive(Serialize)]
struct CoverConfig {
    name: &'static str,
    unique_id: &'static str,
    device_class: &'static str,
    command_topic: &'static str,
    state_topic: &'static str,
    payload_open: &'static str,
    payload_close: &'static str,
    payload_stop: &'static str,
    state_open: &'static str,
    state_opening: &'static str,
    state_closed: &'static str,
    state_closing: &'static str,
    device: Device,
}

#[derive(Serialize)]
struct BinarySensorConfig {
    name: &'static str,
    unique_id: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_class: Option<&'static str>,
    state_topic: &'static str,
    payload_on: &'static str,
    payload_off: &'static str,
    device: Device,
}

#[derive(Serialize)]
struct ButtonConfig {
    name: &'static str,
    unique_id: &'static str,
    command_topic: &'static str,
    payload_press: &'static str,
    device: Device,
}

#[derive(Serialize)]
struct SwitchConfig {
    name: &'static str,
    unique_id: &'static str,
    command_topic: &'static str,
    state_topic: &'static str,
    payload_on: &'static str,
    payload_off: &'static str,
    device: Device,
}

fn json(config: &impl Serialize) -> Vec<u8> {
    // Serializing these plain structs of strings can't fail.
    serde_json::to_vec(config).unwrap()
}

/// Every discovery `(topic, payload)` to publish retained: the gate cover,
/// the wind-lock sensor, the barrier bypass switch, the wind-lock reset
/// button, and an empty payload that removes the retired spare-input sensor.
/// `sw_version` is shown as the device's firmware version in HA.
pub fn configs(sw_version: &'static str) -> Vec<(&'static str, Vec<u8>)> {
    let device = Device {
        identifiers: [NODE_ID],
        name: "Gate",
        manufacturer: "mqtt_gate",
        model: "ESP32-S3 Gate Controller",
        sw_version,
    };

    let cover = CoverConfig {
        name: "gate",
        unique_id: "mqtt_gate_cover",
        device_class: "gate",
        command_topic: COVER_COMMAND_TOPIC,
        state_topic: COVER_STATE_TOPIC,
        payload_open: OPEN_PAYLOAD,
        payload_close: CLOSE_PAYLOAD,
        payload_stop: STOP_PAYLOAD,
        state_open: OPEN_STATE,
        state_opening: OPENING_STATE,
        state_closed: CLOSED_STATE,
        state_closing: CLOSING_STATE,
        device,
    };

    let wind = BinarySensorConfig {
        name: "Wind Monitor Input",
        unique_id: "mqtt_gate_wind",
        device_class: Some("safety"),
        state_topic: WIND_STATE_TOPIC,
        payload_on: ON_PAYLOAD,
        payload_off: OFF_PAYLOAD,
        device,
    };

    // Arms the one-shot light barrier bypass (see
    // `gate_core::Controls::set_barrier_bypass`); turns itself off again
    // after the next movement.
    let barrier_bypass = SwitchConfig {
        name: "Light Barrier Bypass",
        unique_id: "mqtt_gate_barrier_bypass",
        command_topic: BARRIER_BYPASS_COMMAND_TOPIC,
        state_topic: BARRIER_BYPASS_STATE_TOPIC,
        payload_on: ON_PAYLOAD,
        payload_off: OFF_PAYLOAD,
        device,
    };

    // Clears the wind lock (see `gate_core::Controls::reset_wind_lock`)
    // without commanding any gate movement itself.
    let reset_anemometer = ButtonConfig {
        name: "Reset Windmonitor",
        unique_id: "mqtt_gate_reset_anemometer",
        command_topic: BUTTON_COMMAND_TOPIC,
        payload_press: RESET_WIND_LOCK_PAYLOAD,
        device,
    };

    alloc::vec![
        (COVER_CONFIG_TOPIC, json(&cover)),
        (WIND_CONFIG_TOPIC, json(&wind)),
        // An empty retained config removes the old spare-input entity from HA.
        (SPARE_INPUT_CONFIG_TOPIC, Vec::new()),
        (BARRIER_BYPASS_CONFIG_TOPIC, json(&barrier_bypass)),
        (BUTTON_CONFIG_TOPIC, json(&reset_anemometer)),
    ]
}

#[cfg(test)]
mod tests;
