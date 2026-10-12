//! Firmware glue around `gate_core`: the one static [`Controls`] instance,
//! MQTT state publishing, flash persistence and the gate task. All control
//! logic (leaf sequencing, impulse cycle, light barriers, wind lock) lives in
//! the hardware-independent `gate-core` crate, where it's unit-tested.

use esp_hal::gpio::Output;
use gate_core::{Controls, CoverState, Gate, LeafTiming, Motor, Snapshot};
use log::warn;
use mqtt_async_embedded::packet::QoS;

use crate::app::mqtt_topics::*;
use crate::infra::mqtt_client;
use crate::infra::storage::{self, GateState, SharedFlash};
use crate::physical::gate_config::{END_STOP_OVERRUN, REVERSE_PAUSE};

pub struct MotorSettings {
    /// Already driven low - created first thing in `main` so the relays never
    /// float while the rest of the system boots.
    pub open_pin: Output<'static>,
    pub close_pin: Output<'static>,
    pub timing: LeafTiming,
    pub initial_position: u8, // 0..255, restored from flash so a reboot resumes where it left off
}

static CONTROLS: Controls = Controls::new();

// Plain `fn()`s so they can be handed to `physical::inputs` and called from
// `app::mqtt_handler`. Never block; see `gate_core::Controls` for semantics.

pub fn open() {
    CONTROLS.open();
}

pub fn close() {
    CONTROLS.close();
}

pub fn stop() {
    CONTROLS.stop();
}

pub fn impulse() {
    CONTROLS.impulse();
}

pub fn wind_trigger() {
    CONTROLS.wind_trigger();
}

pub fn reset_wind_lock() {
    CONTROLS.reset_wind_lock();
}

pub fn barrier1_set(active: bool) {
    CONTROLS.barrier1_set(active);
}

pub fn barrier2_set(active: bool) {
    CONTROLS.barrier2_set(active);
}

/// Publishes the current cover and wind-lock state (retained). Never blocks:
/// if MQTT is down the message is simply queued or dropped, and the full
/// state is republished on the next (re)connect anyway.
pub fn publish_state() {
    let cover = match CONTROLS.cover_state() {
        CoverState::Open => OPEN_STATE,
        CoverState::Opening => OPENING_STATE,
        CoverState::Closed => CLOSED_STATE,
        CoverState::Closing => CLOSING_STATE,
    };
    mqtt_client::publish(
        COVER_STATE_TOPIC,
        cover.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    );

    let wind = if CONTROLS.wind_locked() {
        ON_PAYLOAD
    } else {
        OFF_PAYLOAD
    };
    mqtt_client::publish(
        WIND_STATE_TOPIC,
        wind.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    );
}

fn motor(settings: MotorSettings) -> Motor<Output<'static>> {
    Motor::new(
        settings.open_pin,
        settings.close_pin,
        settings.timing,
        END_STOP_OVERRUN,
        REVERSE_PAUSE,
        settings.initial_position,
    )
}

fn to_gate_state(snapshot: Snapshot) -> GateState {
    GateState {
        leaf_a_position: snapshot.leaf_a_position,
        leaf_b_position: snapshot.leaf_b_position,
        wind_locked: snapshot.wind_locked,
    }
}

/// Runs the gate forever, persisting position/wind lock to flash after every
/// command whenever they changed - so a power loss never loses track of where
/// the gate was or whether the wind lock was engaged.
#[embassy_executor::task]
pub async fn task(
    leaf_a: MotorSettings,
    leaf_b: MotorSettings,
    flash: &'static SharedFlash,
    initial_wind_locked: bool,
) {
    let mut gate = Gate::new(
        &CONTROLS,
        motor(leaf_a),
        motor(leaf_b),
        initial_wind_locked,
        publish_state,
    );
    // What's in flash right now, so unchanged state isn't rewritten.
    let mut saved = to_gate_state(gate.snapshot());

    loop {
        let state = to_gate_state(gate.step().await);
        if state == saved {
            continue;
        }
        match storage::save_gate_state(&mut *flash.lock().await, &state).await {
            Ok(()) => saved = state,
            Err(err) => warn!("Failed to persist gate state: {err:?}"),
        }
    }
}
