//! Provisioned device configuration: Wi-Fi credentials and MQTT broker settings,
//! persisted to flash (see `crate::infra::storage`) and collected via the provisioning
//! HTTP form (see `crate::infra::provisioning_http`) when none is stored yet.

use alloc::string::String;
use serde::{Deserialize, Serialize};

pub const MAX_SSID_LEN: usize = 32;
pub const MAX_PSK_LEN: usize = 64;
pub const MAX_HOST_LEN: usize = 128;
pub const MAX_CRED_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
}

impl sequential_storage::map::PostcardValue<'_> for AppConfig {}

/// Raw fields as submitted by the provisioning HTML form.
#[derive(Debug, Deserialize)]
pub struct SubmittedForm {
    pub ssid: String,
    #[serde(default)]
    pub wifi_password: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    #[serde(default)]
    pub mqtt_username: String,
    #[serde(default)]
    pub mqtt_password: String,
}

impl SubmittedForm {
    /// Validates field lengths and turns blank optional fields into `None`.
    ///
    /// Returns a human-readable error to show back in the re-rendered form on failure.
    pub fn into_config(self) -> Result<AppConfig, &'static str> {
        if self.ssid.is_empty() {
            return Err("SSID must not be empty");
        }
        if self.ssid.len() > MAX_SSID_LEN {
            return Err("SSID is too long");
        }
        if self.wifi_password.len() > MAX_PSK_LEN {
            return Err("Wi-Fi password is too long");
        }
        if self.mqtt_host.is_empty() {
            return Err("MQTT host must not be empty");
        }
        if self.mqtt_host.len() > MAX_HOST_LEN {
            return Err("MQTT host is too long");
        }
        if self.mqtt_username.len() > MAX_CRED_LEN || self.mqtt_password.len() > MAX_CRED_LEN {
            return Err("MQTT credentials are too long");
        }

        Ok(AppConfig {
            wifi_ssid: self.ssid,
            wifi_password: self.wifi_password,
            mqtt_host: self.mqtt_host,
            mqtt_port: self.mqtt_port,
            mqtt_username: (!self.mqtt_username.is_empty()).then_some(self.mqtt_username),
            mqtt_password: (!self.mqtt_password.is_empty()).then_some(self.mqtt_password),
        })
    }
}
