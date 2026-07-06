//! Application logic: what the device actually does with the MQTT connection
//! provided by `crate::infra::mqtt_client`.

pub mod discovery;
pub mod mqtt_handler;
pub mod mqtt_topics;
