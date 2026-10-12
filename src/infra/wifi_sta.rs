//! Normal-operation Wi-Fi station: joins the network from the persisted `AppConfig`
//! using DHCP, and keeps reconnecting whenever the connection drops.

use embassy_net::{Config, DhcpConfig};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, AuthenticationMethod, WifiController, WifiError};

use crate::infra::config::AppConfig;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);

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

/// Keeps the station connected for the lifetime of the program: connects,
/// waits for a disconnect, and reconnects - retrying forever if the network
/// is unavailable. Nothing local (gate, inputs) waits on this.
#[embassy_executor::task]
pub async fn connection_task(mut controller: WifiController<'static>) -> ! {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                log::info!("Wi-Fi connected");
                let _ = controller.wait_for_disconnect_async().await;
                log::warn!("Wi-Fi disconnected");
            }
            Err(err) => log::warn!("Wi-Fi connect failed: {err:?}"),
        }
        Timer::after(RECONNECT_DELAY).await;
    }
}

/// DHCP-client network config for the station interface.
pub fn net_config() -> Config {
    Config::dhcpv4(DhcpConfig::default())
}
