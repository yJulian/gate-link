//! Inbound MQTT messages driving a real `gate_core::Gate`, and the state
//! messages it reports back.

use core::convert::Infallible;
use core::pin::pin;
use core::task::Poll;
use std::time::Duration as StdDuration;

use embassy_futures::poll_once;
use embassy_time::Duration;
use embedded_hal::digital::{ErrorType, OutputPin};
use gate_core::{Gate, LeafTiming, Motor};

use super::*;

struct Relay;

impl ErrorType for Relay {
    type Error = Infallible;
}

impl OutputPin for Relay {
    fn set_low(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}

const TIMING: LeafTiming = LeafTiming {
    open_initial_delay: Duration::from_secs(0),
    open_duration: Duration::from_secs(15),
    close_initial_delay: Duration::from_secs(0),
    close_duration: Duration::from_secs(15),
};

fn gate(controls: &Controls, position: u8) -> Gate<'_, Relay> {
    let motor = || {
        Motor::new(
            Relay,
            Relay,
            TIMING,
            Duration::from_secs(1),
            Duration::from_secs(1),
            position,
        )
    };
    Gate::new(controls, motor(), motor(), false, || {})
}

/// Delivers one MQTT message and lets the gate react to it (without any
/// simulated time passing).
fn deliver(controls: &Controls, gate: &mut Gate<'_, Relay>, topic: &str, payload: &str) {
    Command::parse(topic, payload.as_bytes())
        .unwrap()
        .apply(controls);
    // The command takes effect on the gate's first poll; a movement then
    // keeps running, anything else settles right away.
    let _ = poll_once(pin!(gate.step()));
}

fn state(controls: &Controls, topic: &str) -> &'static str {
    state_messages(controls)
        .into_iter()
        .find(|(t, _)| *t == topic)
        .unwrap()
        .1
}

#[test]
fn parses_every_command() {
    let cases = [
        (COVER_COMMAND_TOPIC, "OPEN", Command::Open),
        (COVER_COMMAND_TOPIC, "CLOSE", Command::Close),
        (COVER_COMMAND_TOPIC, "STOP", Command::Stop),
        (
            BUTTON_COMMAND_TOPIC,
            "reset_anemometer",
            Command::ResetWindLock,
        ),
        (
            BARRIER_BYPASS_COMMAND_TOPIC,
            "ON",
            Command::SetBarrierBypass(true),
        ),
        (
            BARRIER_BYPASS_COMMAND_TOPIC,
            "OFF",
            Command::SetBarrierBypass(false),
        ),
    ];
    for (topic, payload, expected) in cases {
        assert_eq!(Command::parse(topic, payload.as_bytes()), Ok(expected));
    }
}

#[test]
fn rejects_unknown_topics_and_payloads() {
    assert_eq!(
        Command::parse("mqtt_gate/other/set", b"OPEN"),
        Err(CommandError::UnknownTopic)
    );
    // Payloads are case-sensitive, exactly as HA sends them.
    for payload in [&b"open"[..], b"", b" OPEN", b"\xff\xfe"] {
        assert_eq!(
            Command::parse(COVER_COMMAND_TOPIC, payload),
            Err(CommandError::UnknownPayload)
        );
    }
    assert_eq!(
        Command::parse(BARRIER_BYPASS_COMMAND_TOPIC, b"TOGGLE"),
        Err(CommandError::UnknownPayload)
    );
}

#[test]
fn every_subscribed_topic_is_understood() {
    for topic in COMMAND_TOPICS {
        let understood = ["OPEN", "reset_anemometer", "ON"]
            .iter()
            .any(|payload| Command::parse(topic, payload.as_bytes()).is_ok());
        assert!(understood, "{topic}");
    }
}

#[test]
fn state_after_boot() {
    let controls = Controls::new();
    let _gate = gate(&controls, 0);
    assert_eq!(
        state_messages(&controls),
        [
            (COVER_STATE_TOPIC, "closed"),
            (WIND_STATE_TOPIC, "OFF"),
            (BARRIER_BYPASS_STATE_TOPIC, "OFF"),
        ]
    );
}

#[test]
fn open_and_stop_over_mqtt() {
    let controls = Controls::new();
    let mut gate = gate(&controls, 0);

    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "OPEN");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "opening");

    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "STOP");
    // Stopped right after starting: still (barely) closed.
    assert_ne!(state(&controls, COVER_STATE_TOPIC), "opening");
}

#[test]
fn close_over_mqtt() {
    let controls = Controls::new();
    let mut gate = gate(&controls, 255);
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "open");

    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "CLOSE");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "closing");
}

#[test]
fn barrier_bypass_switch_round_trip() {
    let controls = Controls::new();
    let mut gate = gate(&controls, 0);

    deliver(&controls, &mut gate, BARRIER_BYPASS_COMMAND_TOPIC, "ON");
    assert_eq!(state(&controls, BARRIER_BYPASS_STATE_TOPIC), "ON");
    assert!(controls.warning_light_active());

    deliver(&controls, &mut gate, BARRIER_BYPASS_COMMAND_TOPIC, "OFF");
    assert_eq!(state(&controls, BARRIER_BYPASS_STATE_TOPIC), "OFF");
    assert!(!controls.warning_light_active());
}

#[test]
fn barrier_bypass_lets_mqtt_close_through_broken_barriers() {
    let controls = Controls::new();
    let mut gate = gate(&controls, 255);
    controls.barrier1_set(true);

    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "CLOSE");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "open", "blocked");

    deliver(&controls, &mut gate, BARRIER_BYPASS_COMMAND_TOPIC, "ON");
    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "CLOSE");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "closing");
}

#[test]
fn wind_lock_ignores_mqtt_until_reset() {
    let controls = Controls::new();
    let mut gate = gate(&controls, 255);
    controls.wind_trigger();
    let _ = poll_once(pin!(gate.step()));
    assert_eq!(state(&controls, WIND_STATE_TOPIC), "ON");

    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "CLOSE");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "open");

    deliver(
        &controls,
        &mut gate,
        BUTTON_COMMAND_TOPIC,
        "reset_anemometer",
    );
    assert_eq!(state(&controls, WIND_STATE_TOPIC), "OFF");
    deliver(&controls, &mut gate, COVER_COMMAND_TOPIC, "CLOSE");
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "closing");
}

#[test]
fn a_command_never_blocks_the_caller() {
    // The MQTT handler must not stall when the gate task is busy: sending
    // many commands without the gate consuming any of them returns at once.
    let controls = Controls::new();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        Command::Open.apply(&controls);
        Command::Stop.apply(&controls);
    }
    assert!(started.elapsed() < StdDuration::from_secs(1));
    let mut gate = gate(&controls, 0);
    // Latest wins: only the final STOP is executed.
    assert!(matches!(poll_once(pin!(gate.step())), Poll::Ready(_)));
    assert_eq!(state(&controls, COVER_STATE_TOPIC), "closed");
}
