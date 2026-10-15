//! Publishes the Home Assistant MQTT Discovery configs (built and tested in
//! `app_core::discovery`) so the gate's entities show up without manual YAML.
//!
//! Configs are published retained, and - together with the current gate,
//! wind-lock and barrier-bypass state - republished after every broker
//! (re)connect, since anything published while offline may have been dropped.

use mqtt_async_embedded::packet::QoS;

use crate::infra::mqtt_client;
use crate::physical;

pub fn publish_all() {
    for (topic, payload) in app_core::discovery::configs(env!("CARGO_PKG_VERSION")) {
        mqtt_client::publish(topic, payload, QoS::AtLeastOnce, true);
    }
}

#[embassy_executor::task]
pub async fn task() -> ! {
    loop {
        mqtt_client::wait_connected().await;
        publish_all();
        physical::gate::publish_state();
    }
}
