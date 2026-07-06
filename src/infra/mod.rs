//! Low-level infrastructure: Wi-Fi, provisioning, storage, and the MQTT transport.
//!
//! These modules know how to talk to the outside world but not what any of it
//! means — that's `crate::app`'s job. In particular, `mqtt_client` only owns the
//! broker connection; see its docs for how incoming/outgoing messages cross into
//! the application layer.

pub mod config;
pub mod dhcp_server;
pub mod mqtt_client;
pub mod provisioning_http;
pub mod reset_button;
pub mod storage;
pub mod wifi_ap;
pub mod wifi_sta;
