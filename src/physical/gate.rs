//! Firmware glue around `gate_core`: the one static [`Controls`] instance,
//! MQTT state publishing, flash persistence, the gate task and the warning
//! light task. All control logic (leaf sequencing, impulse cycle, light
//! barriers and their bypass, wind lock, when the warning light blinks)
//! lives in the hardware-independent `gate-core` crate, where it's
//! unit-tested.

use app_core::mqtt::{Command, state_messages};
use app_core::storage::GateState;
use esp_hal::gpio::Output;
use gate_core::{Controls, Gate, LeafTiming, Motor, Snapshot};
use log::warn;
use mqtt_async_embedded::packet::QoS;

use crate::infra::mqtt_client;
use crate::infra::storage::{self, SharedFlash};
use crate::physical::gate_config::{END_STOP_OVERRUN, REVERSE_PAUSE, WARNING_LIGHT_HALF_PERIOD};

pub struct MotorSettings {
    /// Already driven low - created first thing in `main` so the relays never
    /// float while the rest of the system boots.
    pub open_pin: Output<'static>,
    pub close_pin: Output<'static>,
    pub timing: LeafTiming,
    pub initial_position: u8, // 0..255, restored from flash so a reboot resumes where it left off
}

static CONTROLS: Controls = Controls::new();

/// Executes an MQTT command (from `app::mqtt_handler`). Never blocks.
pub fn execute(command: Command) {
    command.apply(&CONTROLS);
    if let Command::SetBarrierBypass(_) = command {
        // Doesn't go through the gate task (until it's used up), so publish
        // the new switch state right away.
        publish_state();
    }
}

// Plain `fn()`s so they can be handed to `physical::inputs`. Never block;
// see `gate_core::Controls` for semantics.

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

/// Local bypass input: a press only ever arms the bypass.
pub fn arm_barrier_bypass() {
    execute(Command::SetBarrierBypass(true));
}

/// Publishes the current cover, wind-lock and barrier-bypass state
/// (retained). Never blocks:
/// if MQTT is down the message is simply queued or dropped, and the full
/// state is republished on the next (re)connect anyway.
pub fn publish_state() {
    for (topic, payload) in state_messages(&CONTROLS) {
        mqtt_client::publish(topic, payload.as_bytes(), QoS::AtLeastOnce, true);
    }
}

/// Blinks the warning light while the gate moves or the barrier bypass is
/// armed (see `gate_core::warning_light`).
#[embassy_executor::task]
pub async fn warning_light_task(pin: Output<'static>) -> ! {
    gate_core::warning_light(&CONTROLS, pin, WARNING_LIGHT_HALF_PERIOD).await
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
