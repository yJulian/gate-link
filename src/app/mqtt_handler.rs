//! The single place where incoming MQTT messages are handled.
//!
//! `crate::infra::mqtt_client` owns the broker connection and only deals in raw
//! bytes; this task is where those bytes get meaning. Any module - including
//! future ones unrelated to this file - can *send* an MQTT message by calling
//! `crate::infra::mqtt_client::publish` directly, but *receiving* is centralized
//! here, so there's one place to look for how the device reacts to the broker.
//! Which topic/payload means which command is defined (and tested) in
//! `app_core::mqtt`.

use app_core::mqtt::{COMMAND_TOPICS, Command};
use log::warn;
use mqtt_async_embedded::packet::QoS;

use crate::infra::mqtt_client;
use crate::physical;

#[embassy_executor::task]
pub async fn task() -> ! {
    for topic in COMMAND_TOPICS {
        mqtt_client::subscribe(topic, QoS::AtLeastOnce).await;
    }

    loop {
        let message = mqtt_client::receive().await;
        match Command::parse(&message.topic, &message.payload) {
            Ok(command) => physical::gate::execute(command),
            Err(err) => warn!(
                "[{}] {err:?}: {:?}",
                message.topic,
                core::str::from_utf8(&message.payload).unwrap_or("<binary>")
            ),
        }
    }
}
