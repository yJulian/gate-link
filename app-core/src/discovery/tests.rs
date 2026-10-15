//! Discovery configs checked against what the rest of the firmware actually
//! does: every command HA can send is understood, every state HA expects is
//! published, and every config fits through the MQTT client's buffer.

use std::collections::BTreeSet;
use std::string::String;
use std::vec::Vec;

use gate_core::Controls;
use mqtt_async_embedded::packet::{EncodePacket, Publish, QoS};
use serde_json::{Map, Value};

use super::*;
use crate::mqtt::{Command, MQTT_BUFFER_SIZE, state_messages};

fn entities() -> Vec<(&'static str, Map<String, Value>)> {
    configs("1.2.3")
        .into_iter()
        .filter(|(_, payload)| !payload.is_empty())
        .map(|(topic, payload)| match serde_json::from_slice(&payload) {
            Ok(Value::Object(config)) => (topic, config),
            other => panic!("{topic}: not a JSON object: {other:?}"),
        })
        .collect()
}

fn text<'a>(config: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    config.get(key).and_then(Value::as_str)
}

#[test]
fn every_config_fits_into_one_mqtt_packet() {
    for (topic, payload) in configs("1.2.3-long-version-string") {
        let mut buf = [0u8; MQTT_BUFFER_SIZE];
        let packet = Publish {
            topic,
            qos: QoS::AtLeastOnce,
            retain: true,
            payload: &payload,
            packet_id: Some(1),
        };
        let encoded = packet.encode(&mut buf);
        assert!(
            encoded.is_ok(),
            "{topic}: {} byte payload doesn't fit the {MQTT_BUFFER_SIZE} byte MQTT buffer",
            payload.len()
        );
    }
}

#[test]
fn config_topics_follow_the_discovery_scheme() {
    let mut seen = BTreeSet::new();
    for (topic, _) in configs("1") {
        let parts: Vec<_> = topic.split('/').collect();
        assert_eq!(parts.len(), 4, "{topic}");
        assert_eq!(parts[0], "homeassistant");
        assert!(
            ["cover", "binary_sensor", "switch", "button"].contains(&parts[1]),
            "{topic}"
        );
        assert_eq!(parts[3], "config");
        assert!(seen.insert(topic), "duplicate {topic}");
    }
}

#[test]
fn entities_are_unique_and_belong_to_one_device() {
    let mut ids = BTreeSet::new();
    for (topic, config) in entities() {
        assert!(text(&config, "name").is_some(), "{topic}");
        let id = text(&config, "unique_id").unwrap();
        assert!(ids.insert(id.to_string()), "duplicate unique_id {id}");

        let device = &config["device"];
        assert_eq!(device["identifiers"], serde_json::json!([NODE_ID]));
        assert_eq!(device["sw_version"], "1.2.3");
    }
}

#[test]
fn every_command_ha_can_send_is_understood() {
    let mut checked = 0;
    for (topic, config) in entities() {
        let Some(command_topic) = text(&config, "command_topic") else {
            continue;
        };
        let expected: &[(&str, Command)] = match topic.split('/').nth(1).unwrap() {
            "cover" => &[
                ("payload_open", Command::Open),
                ("payload_close", Command::Close),
                ("payload_stop", Command::Stop),
            ],
            "button" => &[("payload_press", Command::ResetWindLock)],
            "switch" => &[
                ("payload_on", Command::SetBarrierBypass(true)),
                ("payload_off", Command::SetBarrierBypass(false)),
            ],
            other => panic!("unexpected command entity {other}"),
        };
        for (key, command) in expected {
            let payload = text(&config, key).unwrap_or_else(|| panic!("{topic}: no {key}"));
            assert_eq!(
                Command::parse(command_topic, payload.as_bytes()),
                Ok(*command),
                "{topic}: {key}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 6);
}

#[test]
fn every_state_topic_is_published_with_values_ha_understands() {
    let controls = Controls::new();
    controls.set_barrier_bypass(true);
    controls.wind_trigger();
    let published = state_messages(&controls);

    let mut checked = 0;
    for (topic, config) in entities() {
        let Some(state_topic) = text(&config, "state_topic") else {
            continue;
        };
        let (_, value) = published
            .iter()
            .find(|(t, _)| *t == state_topic)
            .unwrap_or_else(|| panic!("{topic}: {state_topic} is never published"));
        let allowed: Vec<&str> = [
            "state_open",
            "state_opening",
            "state_closed",
            "state_closing",
            "payload_on",
            "payload_off",
        ]
        .iter()
        .filter_map(|key| text(&config, key))
        .collect();
        assert!(
            allowed.contains(value),
            "{topic}: {value} not in {allowed:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[test]
fn retired_spare_input_entity_is_removed() {
    let removal = configs("1")
        .into_iter()
        .find(|(topic, _)| *topic == SPARE_INPUT_CONFIG_TOPIC)
        .unwrap();
    assert!(removal.1.is_empty());
}
