//! Provisioning-mode Wi-Fi access point: open (passwordless) hotspot with a static
//! IP, serving the config form (`crate::infra::provisioning_http`) and a small DHCP server
//! (`crate::infra::dhcp_server`) for joining clients.

use embassy_net::{Config, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use esp_radio::wifi::ap::AccessPointConfig;
use esp_radio::wifi::{self, WifiController, WifiError};
use heapless::Vec;

/// SSID advertised by the open provisioning hotspot.
pub const AP_SSID: &str = "mqtt-gate-setup";
pub const AP_IP_OCTETS: [u8; 4] = [192, 168, 2, 1];
pub const AP_PREFIX_LEN: u8 = 24;

/// The AP's own address, as an `embassy_net`/smoltcp type (for the static IP config).
pub const AP_IP: Ipv4Address = Ipv4Address::new(
    AP_IP_OCTETS[0],
    AP_IP_OCTETS[1],
    AP_IP_OCTETS[2],
    AP_IP_OCTETS[3],
);

/// Puts the Wi-Fi radio into (open) access-point mode.
pub fn configure(controller: &mut WifiController<'_>) -> Result<(), WifiError> {
    let ap_config = AccessPointConfig::default().with_ssid(AP_SSID);
    controller.set_config(&wifi::Config::AccessPoint(ap_config))
}

/// Static IP network config for the access-point interface.
pub fn net_config() -> Config {
    Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(AP_IP, AP_PREFIX_LEN),
        gateway: None,
        dns_servers: Vec::new(),
    })
}
