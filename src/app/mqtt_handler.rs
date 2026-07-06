//! The single place where incoming MQTT messages are handled.
//!
//! `crate::infra::mqtt_client` owns the broker connection and only deals in raw
//! bytes; this task is where those bytes get meaning. Any module - including
//! future ones unrelated to this file - can *send* an MQTT message by calling
//! `crate::infra::mqtt_client::publish` directly, but *receiving* is centralized
//! here so there's one place to look for how the device reacts to the broker.

use log::{info, warn};
use serde_json::Value;

use crate::infra::mqtt_client;

#[embassy_executor::task]
pub async fn task() -> ! {
    loop {
        let message = mqtt_client::receive().await;
        on_message(&message.topic, &message.payload);
    }
}

/// Central callback for every incoming MQTT message.
fn on_message(topic: &str, payload: &[u8]) {
    match serde_json::from_slice::<Value>(payload) {
        Ok(json) => info!("[{topic}] {json}"),
        Err(err) => warn!("[{topic}] payload is not valid JSON: {err}"),
    }

    // Add topic-specific handling here as it's needed.
}
