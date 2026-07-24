//! The single place where incoming MQTT messages are handled.
//!
//! `crate::infra::mqtt_client` owns the broker connection and only deals in raw
//! bytes; this task is where those bytes get meaning. Any module - including
//! future ones unrelated to this file - can *send* an MQTT message by calling
//! `crate::infra::mqtt_client::publish` directly, but *receiving* is centralized
//! here, so there's one place to look for how the device reacts to the broker.

use log::warn;

use mqtt_async_embedded::packet::QoS;

use crate::app::mqtt_topics::*;
use crate::infra::mqtt_client;

use crate::physical::gate::{close, open, reset_wind_lock, stop};

#[embassy_executor::task]
pub async fn task() -> ! {
    // Subscribe to topics
    mqtt_client::subscribe(COVER_COMMAND_TOPIC, QoS::AtLeastOnce).await;
    mqtt_client::subscribe(BUTTON_COMMAND_TOPIC, QoS::AtLeastOnce).await;

    loop {
        let message = mqtt_client::receive().await;
        on_message(&message.topic, &message.payload);
    }
}

/// Central callback for every incoming MQTT message.
fn on_message(topic: &str, payload: &[u8]) {
    match topic {
        COVER_COMMAND_TOPIC => {
            let text = core::str::from_utf8(payload).unwrap_or("");
            match text {
                OPEN_PAYLOAD => open(),
                CLOSE_PAYLOAD => close(),
                STOP_PAYLOAD => stop(),
                _ => warn!("[{topic}] unhandled payload: {text}"),
            }
        }
        BUTTON_COMMAND_TOPIC => {
            let text = core::str::from_utf8(payload).unwrap_or("");
            match text {
                RESET_WIND_LOCK_PAYLOAD => reset_wind_lock(),
                _ => warn!("[{topic}] unhandled payload: {text}"),
            }
        }
        _ => warn!("[{topic}] unhandled topic"),
    }
}
