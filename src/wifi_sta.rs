//! Normal-operation Wi-Fi station: joins the network from the persisted `AppConfig`
//! using DHCP.

use embassy_net::{Config, DhcpConfig};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, AuthenticationMethod, WifiController, WifiError};

use crate::config::AppConfig;

/// Puts the Wi-Fi radio into station mode using the persisted SSID/password.
///
/// Only open and WPA2-Personal networks are supported (the common case for a
/// provisioning form with a single password field); WPA3-only, WEP, and enterprise
/// networks aren't.
pub fn configure(controller: &mut WifiController<'_>, cfg: &AppConfig) -> Result<(), WifiError> {
    let mut station_config = StationConfig::default()
        .with_ssid(cfg.wifi_ssid.clone())
        .with_password(cfg.wifi_password.clone());

    if cfg.wifi_password.is_empty() {
        station_config = station_config.with_auth_method(AuthenticationMethod::None);
    }

    controller.set_config(&wifi::Config::Station(station_config))
}

/// DHCP-client network config for the station interface.
pub fn net_config() -> Config {
    Config::dhcpv4(DhcpConfig::default())
}
