use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::AnyPin;
use esp_storage::FlashStorage;
use log::warn;
use mqtt_async_embedded::packet::QoS;

use crate::app::mqtt_topics::*;
use crate::infra::mqtt_client;
use crate::infra::storage::{self, GateState};
use crate::physical::motor::Motor;

#[derive(Clone, Copy)]
pub(crate) enum GateCommand {
    Open,
    Close,
    Stop,
}

pub struct MotorSettings {
    pub open_pin: AnyPin<'static>,
    pub close_pin: AnyPin<'static>,
    pub duration: u8,         // Seconds needed to travel the full 0..255 range
    pub initial_position: u8, // 0..255, restored from flash so a reboot resumes where it left off
}

static COMMANDS: Channel<CriticalSectionRawMutex, GateCommand, 1> = Channel::new();

const DIR_IDLE: u8 = 0;
const DIR_OPENING: u8 = 1;
const DIR_CLOSING: u8 = 2;

/// What the motors are doing right now. `DIR_IDLE` whenever no command is in
/// flight - used by [`impulse`] to decide "stop" vs "start" and by the light
/// barriers to decide whether they're relevant.
static CURRENT_DIRECTION: AtomicU8 = AtomicU8::new(DIR_IDLE);

/// The most recent non-idle direction, kept around after the motors go idle.
/// [`impulse`] uses this to resume in the opposite direction when the gate
/// was stopped mid-travel. Defaults to "closing" so the very first impulse
/// with an unknown/partial position opens rather than closes.
static LAST_DIRECTION: AtomicU8 = AtomicU8::new(DIR_CLOSING);

const POS_CLOSED: u8 = 0;
const POS_PARTIAL: u8 = 1;
const POS_OPEN: u8 = 2;

/// Coarse classification of the current leaf positions, refreshed after
/// every settle. Closed only when both leaves are fully closed, open only
/// when both are fully open, partial otherwise.
static POSITION: AtomicU8 = AtomicU8::new(POS_CLOSED);

/// Set by the wind guard (anemometer) input, cleared by the dedicated reset
/// input or the `reset_anemometer` MQTT button. While set, the gate is
/// forced open and all normal open/close/stop/impulse inputs are ignored.
static WIND_LOCKED: AtomicBool = AtomicBool::new(false);

/// Light barrier 1: stops the gate immediately regardless of direction, and
/// blocks new open/close commands while the beam stays broken.
static BARRIER1_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Light barrier 2: only relevant while closing - stops/blocks closing while
/// the beam is broken, but never blocks opening.
static BARRIER2_ACTIVE: AtomicBool = AtomicBool::new(false);

fn send(command: GateCommand) {
    if COMMANDS.try_send(command).is_err() {
        warn!("Gate command dropped: a command is already pending");
    }
}

fn wind_locked() -> bool {
    WIND_LOCKED.load(Ordering::Relaxed)
}

/// Queues an OPEN unless light barrier 1 is currently broken - used both by
/// the gated public [`open`] and by the paths that must open regardless of
/// the wind lock (the wind lock forcing the gate open is what barrier 1 is
/// there to override).
fn send_open_unless_barrier1() {
    if !BARRIER1_ACTIVE.load(Ordering::Relaxed) {
        send(GateCommand::Open);
    }
}

pub fn open() {
    if wind_locked() {
        warn!("Gate locked by wind guard - ignoring open()");
        return;
    }
    send_open_unless_barrier1();
}

pub fn close() {
    if wind_locked() {
        warn!("Gate locked by wind guard - ignoring close()");
        return;
    }
    if BARRIER1_ACTIVE.load(Ordering::Relaxed) || BARRIER2_ACTIVE.load(Ordering::Relaxed) {
        warn!("Light barrier active - ignoring close()");
        return;
    }
    send(GateCommand::Close);
}

pub fn stop() {
    if wind_locked() {
        warn!("Gate locked by wind guard - ignoring stop()");
        return;
    }
    send(GateCommand::Stop);
}

/// Local push button and radio remote: press cycle is open -> stop -> close
/// -> stop -> open -> ... If the gate is moving, stop it. If it's idle at a
/// known end, go the other way. If it's idle mid-travel (a previous impulse
/// stopped it there), continue with the opposite of the direction that was
/// interrupted.
pub fn impulse() {
    if wind_locked() {
        warn!("Gate locked by wind guard - ignoring impulse");
        return;
    }
    if CURRENT_DIRECTION.load(Ordering::Relaxed) != DIR_IDLE {
        stop();
        return;
    }
    match POSITION.load(Ordering::Relaxed) {
        POS_CLOSED => open(),
        POS_OPEN => close(),
        _ => match LAST_DIRECTION.load(Ordering::Relaxed) {
            DIR_OPENING => close(),
            _ => open(),
        },
    }
}

/// Wind guard tripped: latches the lock, forces the gate open (bypassing the
/// lock check itself and the normal button/MQTT gating - only barrier 1 can
/// still override this), and republishes state once the command settles.
pub fn wind_trigger() {
    WIND_LOCKED.store(true, Ordering::Relaxed);
    send_open_unless_barrier1();
}

/// Clears the wind lock, from either the dedicated reset input or the
/// `reset_anemometer` MQTT button. Queues a (harmless if already idle) STOP
/// so the task loop republishes/persists the new state promptly.
pub fn reset_wind_lock() {
    WIND_LOCKED.store(false, Ordering::Relaxed);
    send(GateCommand::Stop);
}

/// Light barrier 1 (always stops, regardless of direction).
pub fn barrier1_set(active: bool) {
    BARRIER1_ACTIVE.store(active, Ordering::Relaxed);
    if active {
        send(GateCommand::Stop);
    } else if wind_locked() {
        // Keep pursuing "always open" once the obstruction clears.
        send_open_unless_barrier1();
    }
}

/// Light barrier 2 (only stops closing; opening is never affected).
pub fn barrier2_set(active: bool) {
    BARRIER2_ACTIVE.store(active, Ordering::Relaxed);
    if active && CURRENT_DIRECTION.load(Ordering::Relaxed) == DIR_CLOSING {
        send(GateCommand::Stop);
    }
}

fn set_direction(direction: u8) {
    CURRENT_DIRECTION.store(direction, Ordering::Relaxed);
    LAST_DIRECTION.store(direction, Ordering::Relaxed);
}

async fn publish_cover_state(state: &'static str) {
    mqtt_client::publish(
        COVER_STATE_TOPIC,
        state.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    )
    .await;
}

async fn publish_wind_state(locked: bool) {
    let payload = if locked { ON_PAYLOAD } else { OFF_PAYLOAD };
    mqtt_client::publish(
        WIND_STATE_TOPIC,
        payload.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    )
    .await;
}

/// Re-classifies the current leaf positions, publishes the resulting cover
/// and wind-lock state, and persists both to flash. Called once after every
/// command settles (completes or is interrupted) so a power loss never loses
/// track of where the gate was or whether the wind lock was engaged.
async fn settle_and_persist<'m>(
    left: &Motor<'m>,
    right: &Motor<'m>,
    flash: &mut FlashStorage<'static>,
) {
    CURRENT_DIRECTION.store(DIR_IDLE, Ordering::Relaxed);

    let left_position = left.position();
    let right_position = right.position();
    let position = if left_position == 0 && right_position == 0 {
        POS_CLOSED
    } else if left_position == 255 && right_position == 255 {
        POS_OPEN
    } else {
        POS_PARTIAL
    };
    POSITION.store(position, Ordering::Relaxed);

    let locked = wind_locked();
    publish_cover_state(if position == POS_CLOSED {
        CLOSED_STATE
    } else {
        OPEN_STATE
    })
    .await;
    publish_wind_state(locked).await;

    let state = GateState {
        left_position,
        right_position,
        wind_locked: locked,
    };
    if let Err(err) = storage::save_gate_state(flash, &state).await {
        warn!("Failed to persist gate state: {err:?}");
    }
}

async fn wait_or_interrupt(duration: Duration) -> Option<GateCommand> {
    match select(Timer::after(duration), COMMANDS.receive()).await {
        Either::First(_) => None,
        Either::Second(command) => Some(command),
    }
}

/// Opens both leaves so they reach fully open at the same time: the leaf
/// that needs less time waits for the other to catch up before starting.
async fn run_open<'m>(left: &mut Motor<'m>, right: &mut Motor<'m>) -> Option<GateCommand> {
    let left_remaining = left.remaining_open();
    let right_remaining = right.remaining_open();

    let (longer, shorter, stagger) = if left_remaining >= right_remaining {
        (left, right, left_remaining - right_remaining)
    } else {
        (right, left, right_remaining - left_remaining)
    };

    longer.start_open();
    if let Some(command) = wait_or_interrupt(stagger).await {
        longer.stop();
        return Some(command);
    }

    shorter.start_open();
    if let Some(command) = wait_or_interrupt(shorter.remaining_open()).await {
        longer.stop();
        shorter.stop();
        return Some(command);
    }

    longer.stop();
    shorter.stop();
    None
}

/// Closes both leaves at the same time, so the leaf that needs less time
/// arrives - and stops - well before the other.
async fn run_close<'m>(left: &mut Motor<'m>, right: &mut Motor<'m>) -> Option<GateCommand> {
    left.start_close();
    right.start_close();

    let left_remaining = left.remaining_close();
    let right_remaining = right.remaining_close();

    let (sooner, later, sooner_time, extra) = if left_remaining <= right_remaining {
        (
            left,
            right,
            left_remaining,
            right_remaining - left_remaining,
        )
    } else {
        (
            right,
            left,
            right_remaining,
            left_remaining - right_remaining,
        )
    };

    if let Some(command) = wait_or_interrupt(sooner_time).await {
        sooner.stop();
        later.stop();
        return Some(command);
    }
    sooner.stop();

    if let Some(command) = wait_or_interrupt(extra).await {
        later.stop();
        return Some(command);
    }
    later.stop();
    None
}

#[embassy_executor::task]
pub async fn task(
    left_settings: MotorSettings,
    right_settings: MotorSettings,
    flash: &'static mut FlashStorage<'static>,
    initial_wind_locked: bool,
) {
    let mut left = Motor::new(left_settings);
    let mut right = Motor::new(right_settings);

    WIND_LOCKED.store(initial_wind_locked, Ordering::Relaxed);
    // Establish POSITION/LAST_DIRECTION from the restored positions and
    // publish the current state once, so HA has a fresh retained value
    // immediately after boot.
    settle_and_persist(&left, &right, flash).await;

    // If the wind lock was still engaged when power was lost, keep pursuing
    // "always open" right away instead of waiting for the anemometer to trip
    // again.
    let mut pending = if initial_wind_locked {
        Some(GateCommand::Open)
    } else {
        None
    };

    loop {
        let command = match pending.take() {
            Some(command) => command,
            None => COMMANDS.receive().await,
        };
        pending = match command {
            GateCommand::Open => {
                set_direction(DIR_OPENING);
                publish_cover_state(OPENING_STATE).await;
                run_open(&mut left, &mut right).await
            }
            GateCommand::Close => {
                set_direction(DIR_CLOSING);
                publish_cover_state(CLOSING_STATE).await;
                run_close(&mut left, &mut right).await
            }
            GateCommand::Stop => {
                left.stop();
                right.stop();
                None
            }
        };

        settle_and_persist(&left, &right, flash).await;
    }
}
