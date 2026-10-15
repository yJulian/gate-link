//! The provisioning pages served by the firmware's `infra::provisioning_http`
//! in access-point mode. The form's field names and limits must match
//! [`SubmittedForm`](crate::config::SubmittedForm) - which the tests check.

use alloc::string::String;
use core::fmt::Write as _;

use crate::config::{MAX_CRED_LEN, MAX_HOST_LEN, MAX_PSK_LEN, MAX_SSID_LEN};

/// The config form, optionally with a validation error from the last submit.
pub fn form_page(error: Option<&str>) -> String {
    let mut page = String::new();
    let _ = write!(
        page,
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>mqtt_gate setup</title></head><body>\
         <h1>mqtt_gate setup</h1>"
    );
    if let Some(error) = error {
        let _ = write!(page, "<p style=\"color:red\">{error}</p>");
    }
    let _ = write!(
        page,
        "<form method=\"post\" action=\"/\">\
         <label>Wi-Fi SSID<br><input name=\"ssid\" required maxlength=\"{MAX_SSID_LEN}\"></label><br>\
         <label>Wi-Fi password<br><input name=\"wifi_password\" type=\"password\" maxlength=\"{MAX_PSK_LEN}\"></label><br>\
         <label>MQTT host<br><input name=\"mqtt_host\" required maxlength=\"{MAX_HOST_LEN}\"></label><br>\
         <label>MQTT port<br><input name=\"mqtt_port\" type=\"number\" value=\"1883\" required></label><br>\
         <label>MQTT username (optional)<br><input name=\"mqtt_username\" maxlength=\"{MAX_CRED_LEN}\"></label><br>\
         <label>MQTT password (optional)<br><input name=\"mqtt_password\" type=\"password\" maxlength=\"{MAX_CRED_LEN}\"></label><br>\
         <button type=\"submit\">Save &amp; reboot</button>\
         </form></body></html>"
    );
    page
}

/// Shown after a valid submission, right before the device reboots.
pub const SAVED_PAGE: &str =
    "<!doctype html><html><body><p>Saved. Rebooting into station mode...</p></body></html>";
