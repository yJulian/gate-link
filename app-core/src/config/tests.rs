//! The provisioning path end to end: the served form, a browser's
//! URL-encoded submission, picoserve's form decoder (the one the firmware
//! uses) and validation into an [`AppConfig`].

use std::collections::BTreeSet;
use std::string::{String, ToString};

use picoserve::url_encoded::{UrlEncodedString, deserialize_form};

use super::*;
use crate::provisioning::form_page;

fn decode(body: &str) -> Option<SubmittedForm> {
    deserialize_form(UrlEncodedString(body)).ok()
}

fn submit(body: &str) -> Result<AppConfig, &'static str> {
    decode(body).expect("form decodes").into_config()
}

/// All `name="..."` attributes of the form's inputs.
fn form_field_names() -> BTreeSet<String> {
    form_page(None)
        .split("name=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter(|name| *name != "viewport")
        .map(ToString::to_string)
        .collect()
}

fn form(ssid: &str, password: &str, host: &str, user: &str, pass: &str) -> String {
    format!(
        "ssid={ssid}&wifi_password={password}&mqtt_host={host}&mqtt_port=1883\
         &mqtt_username={user}&mqtt_password={pass}"
    )
}

#[test]
fn form_fields_are_exactly_the_submitted_form_fields() {
    let expected: BTreeSet<String> = [
        "ssid",
        "wifi_password",
        "mqtt_host",
        "mqtt_port",
        "mqtt_username",
        "mqtt_password",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(form_field_names(), expected);
}

#[test]
fn form_length_limits_match_validation() {
    let page = form_page(None);
    for (field, max) in [
        ("ssid", MAX_SSID_LEN),
        ("wifi_password", MAX_PSK_LEN),
        ("mqtt_host", MAX_HOST_LEN),
        ("mqtt_username", MAX_CRED_LEN),
        ("mqtt_password", MAX_CRED_LEN),
    ] {
        let input = page
            .split(&format!("name=\"{field}\""))
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .unwrap();
        assert!(
            input.contains(&format!("maxlength=\"{max}\"")),
            "{field}: {input}"
        );
    }
}

#[test]
fn browser_submission_with_special_characters() {
    // What a browser posts for SSID "My Wifi", password "p@ss w&rd=1",
    // empty MQTT credentials.
    let config = submit(
        "ssid=My+Wifi&wifi_password=p%40ss+w%26rd%3D1&mqtt_host=192.168.1.10\
         &mqtt_port=1883&mqtt_username=&mqtt_password=",
    )
    .unwrap();
    assert_eq!(
        config,
        AppConfig {
            wifi_ssid: "My Wifi".into(),
            wifi_password: "p@ss w&rd=1".into(),
            mqtt_host: "192.168.1.10".into(),
            mqtt_port: 1883,
            mqtt_username: None,
            mqtt_password: None,
        }
    );
}

#[test]
fn utf8_ssid_is_decoded() {
    let config = submit(&form("K%C3%BCche", "", "broker", "", "")).unwrap();
    assert_eq!(config.wifi_ssid, "Küche");
}

#[test]
fn mqtt_credentials_are_kept_when_given() {
    let config = submit(&form("w", "", "broker.local", "ha", "secret")).unwrap();
    assert_eq!(config.mqtt_username.as_deref(), Some("ha"));
    assert_eq!(config.mqtt_password.as_deref(), Some("secret"));
}

#[test]
fn omitted_optional_fields_default_to_empty() {
    let config = submit("ssid=w&mqtt_host=broker&mqtt_port=1883").unwrap();
    assert_eq!(config.wifi_password, "");
    assert_eq!(config.mqtt_username, None);
}

#[test]
fn invalid_port_is_rejected() {
    for port in ["", "abc", "70000", "-1"] {
        let body = format!("ssid=w&mqtt_host=broker&mqtt_port={port}");
        assert!(decode(&body).is_none(), "port {port:?}");
    }
}

#[test]
fn length_limits_are_inclusive() {
    let at = |n: usize| "x".repeat(n);
    assert!(submit(&form(&at(MAX_SSID_LEN), "", "h", "", "")).is_ok());
    assert!(submit(&form(&at(MAX_SSID_LEN + 1), "", "h", "", "")).is_err());
    assert!(submit(&form("w", &at(MAX_PSK_LEN), "h", "", "")).is_ok());
    assert!(submit(&form("w", &at(MAX_PSK_LEN + 1), "h", "", "")).is_err());
    assert!(submit(&form("w", "", &at(MAX_HOST_LEN), "", "")).is_ok());
    assert!(submit(&form("w", "", &at(MAX_HOST_LEN + 1), "", "")).is_err());
    assert!(submit(&form("w", "", "h", &at(MAX_CRED_LEN), &at(MAX_CRED_LEN))).is_ok());
    assert!(submit(&form("w", "", "h", &at(MAX_CRED_LEN + 1), "")).is_err());
    assert!(submit(&form("w", "", "h", "", &at(MAX_CRED_LEN + 1))).is_err());
}

#[test]
fn ssid_limit_counts_bytes_not_characters() {
    // 802.11 limits SSIDs to 32 *bytes*; "ü" is two.
    let ok = "%C3%BC".repeat(MAX_SSID_LEN / 2);
    let too_long = "%C3%BC".repeat(MAX_SSID_LEN / 2 + 1);
    assert!(submit(&form(&ok, "", "h", "", "")).is_ok());
    assert!(submit(&form(&too_long, "", "h", "", "")).is_err());
}

#[test]
fn required_fields_must_not_be_empty() {
    assert_eq!(
        submit(&form("", "", "h", "", "")),
        Err("SSID must not be empty")
    );
    assert_eq!(
        submit(&form("w", "", "", "", "")),
        Err("MQTT host must not be empty")
    );
}

#[test]
fn validation_error_is_shown_in_the_form() {
    let page = form_page(Some("SSID is too long"));
    assert!(page.contains("SSID is too long"));
    assert!(!form_page(None).contains("color:red"));
}
