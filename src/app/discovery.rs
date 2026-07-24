//! Publishes Home Assistant MQTT Discovery configs so the gate, its wind
//! lock sensor and the wind-lock-reset button show up as entities without
//! manual YAML.
//!
//! <https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery>
//!
//! Configs are published retained: Home Assistant (or a broker restart) only
//! has to see them once, not exactly at device boot. The actual state/command
//! wiring lives in `crate::physical::gate` and `crate::app::mqtt_handler`.

use alloc::vec::Vec;

use mqtt_async_embedded::packet::QoS;
use serde::Serialize;

use crate::app::mqtt_topics::*;
use crate::infra::mqtt_client;

#[derive(Serialize, Clone, Copy)]
struct Device {
    identifiers: [&'static str; 1],
    name: &'static str,
    manufacturer: &'static str,
    model: &'static str,
    sw_version: &'static str,
}

const DEVICE: Device = Device {
    identifiers: [NODE_ID],
    name: "Gate",
    manufacturer: "mqtt_gate",
    model: "ESP32 Gate Controller",
    sw_version: env!("CARGO_PKG_VERSION"),
};

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
    device_class: &'static str,
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

async fn publish_config(topic: &'static str, payload: Vec<u8>) {
    mqtt_client::publish(topic, payload, QoS::AtLeastOnce, true).await;
}

/// Publishes the retained discovery config for the gate cover, its contact
/// sensor and the impulse button. Safe to call once at startup; the broker
/// keeps the retained messages around for Home Assistant to pick up whenever
/// it (re)connects.
pub async fn publish_all() {
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
        device: DEVICE,
    };
    publish_config(COVER_CONFIG_TOPIC, serde_json::to_vec(&cover).unwrap()).await;

    let wind = BinarySensorConfig {
        name: "Windwächter",
        unique_id: "mqtt_gate_wind",
        device_class: "safety",
        state_topic: WIND_STATE_TOPIC,
        payload_on: ON_PAYLOAD,
        payload_off: OFF_PAYLOAD,
        device: DEVICE,
    };
    publish_config(WIND_CONFIG_TOPIC, serde_json::to_vec(&wind).unwrap()).await;

    // Clears the wind lock (see `physical::gate::reset_wind_lock`) without
    // commanding any gate movement itself.
    let reset_anemometer = ButtonConfig {
        name: "Windwächter zurücksetzen",
        unique_id: "mqtt_gate_reset_anemometer",
        command_topic: BUTTON_COMMAND_TOPIC,
        payload_press: RESET_WIND_LOCK_PAYLOAD,
        device: DEVICE,
    };
    publish_config(
        BUTTON_CONFIG_TOPIC,
        serde_json::to_vec(&reset_anemometer).unwrap(),
    )
    .await;
}

#[embassy_executor::task]
pub async fn task() {
    publish_all().await;
}
