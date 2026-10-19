//! Gate scenarios on a simulated clock. Each test checks the exact relay
//! switching sequence (pin + millisecond) the firmware would produce.

use core::sync::atomic::{AtomicUsize, Ordering};

use embassy_futures::select::select;
use embassy_time::Duration;

use super::*;
use crate::motor::LeafTiming;
use crate::sim::{self, MockPin, PinLog, off, on};
use crate::warning_light;

/// Leaf A opens first and closes second, leaf B the other way round - the
/// same shape as the installation's `gate_config`.
const LEAF_A: LeafTiming = LeafTiming {
    open_initial_delay: Duration::from_secs(0),
    open_duration: Duration::from_secs(15),
    close_initial_delay: Duration::from_secs(3),
    close_duration: Duration::from_secs(15),
};
const LEAF_B: LeafTiming = LeafTiming {
    open_initial_delay: Duration::from_secs(3),
    open_duration: Duration::from_secs(15),
    close_initial_delay: Duration::from_secs(0),
    close_duration: Duration::from_secs(15),
};
const OVERRUN: Duration = Duration::from_secs(1);
const REVERSE_PAUSE: Duration = Duration::from_secs(1);

const A_OPEN: &str = "A_open";
const A_CLOSE: &str = "A_close";
const B_OPEN: &str = "B_open";
const B_CLOSE: &str = "B_close";

const OPEN: u8 = 255;
const CLOSED: u8 = 0;

/// Long enough for any single command in these tests to finish.
const LIMIT: Duration = Duration::from_secs(60);
/// For steps that must complete without any time passing.
const IMMEDIATELY: Duration = Duration::from_ticks(0);

fn no_notify() {}

fn gate<'c>(
    controls: &'c Controls,
    log: &PinLog,
    a_position: u8,
    b_position: u8,
    wind_locked: bool,
) -> Gate<'c, MockPin> {
    let a = Motor::new(
        log.pin(A_OPEN),
        log.pin(A_CLOSE),
        LEAF_A,
        OVERRUN,
        REVERSE_PAUSE,
        a_position,
    );
    let b = Motor::new(
        log.pin(B_OPEN),
        log.pin(B_CLOSE),
        LEAF_B,
        OVERRUN,
        REVERSE_PAUSE,
        b_position,
    );
    Gate::new(controls, a, b, wind_locked, no_notify)
}

fn snapshot(a: u8, b: u8, wind_locked: bool) -> Snapshot {
    Snapshot {
        leaf_a_position: a,
        leaf_b_position: b,
        wind_locked,
    }
}

#[test]
fn opening_starts_leaf_a_first_then_b_after_its_initial_delay() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    let settled = clock.run_until_ready(gate.step(), LIMIT);

    // Each leaf: initial delay, then 15s run time + 1s end-stop overrun.
    assert_eq!(
        log.take(),
        [
            on(0, A_OPEN),
            on(3000, B_OPEN),
            off(16000, A_OPEN),
            off(19000, B_OPEN),
        ]
    );
    assert_eq!(settled, snapshot(OPEN, OPEN, false));
    assert_eq!(controls.cover_state(), CoverState::Open);
}

#[test]
fn closing_starts_leaf_b_first_then_a_after_its_initial_delay() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.close();
    let settled = clock.run_until_ready(gate.step(), LIMIT);

    assert_eq!(
        log.take(),
        [
            on(0, B_CLOSE),
            on(3000, A_CLOSE),
            off(16000, B_CLOSE),
            off(19000, A_CLOSE),
        ]
    );
    assert_eq!(settled, snapshot(CLOSED, CLOSED, false));
    assert_eq!(controls.cover_state(), CoverState::Closed);
}

#[test]
fn cover_state_reports_movement_including_initial_delay() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);
    let mut step = Box::pin(gate.step());

    controls.close();
    assert!(clock.run_for(&mut step, Duration::from_secs(1)).is_none());
    assert_eq!(controls.cover_state(), CoverState::Closing);
    clock.run_until_ready(step, LIMIT);
    assert_eq!(controls.cover_state(), CoverState::Closed);
}

#[test]
fn leaf_already_at_its_end_does_not_move() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, CLOSED, false);

    controls.open();
    clock.run_until_ready(gate.step(), LIMIT);

    assert_eq!(log.take(), [on(3000, B_OPEN), off(19000, B_OPEN)]);
}

#[test]
fn impulse_opens_when_closed_and_closes_when_open() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.impulse();
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(OPEN, OPEN, false)
    );
    assert_eq!(log.take()[0], on(0, A_OPEN));

    controls.impulse();
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(CLOSED, CLOSED, false)
    );
    // B only stopped at 19s, so it first sits out the 1s reverse pause.
    assert_eq!(log.take()[0], on(20000, B_CLOSE));
}

#[test]
fn impulse_cycle_close_stop_open() {
    // Click -> close, click -> stop (still running), click -> open.
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.impulse();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    assert_eq!(controls.cover_state(), CoverState::Closing);

    controls.impulse();
    let stopped = clock.run_until_ready(step, IMMEDIATELY);
    // B closed for 5s, A for 2s (it started 3s later) - of 15s each.
    assert_eq!(stopped, snapshot(255 - 34, 255 - 85, false));
    assert_eq!(
        log.take(),
        [
            on(0, B_CLOSE),
            on(3000, A_CLOSE),
            off(5000, A_CLOSE),
            off(5000, B_CLOSE),
        ]
    );
    assert!(!log.any_high());
    assert_eq!(controls.cover_state(), CoverState::Open); // partially open

    controls.impulse();
    let opened = clock.run_until_ready(gate.step(), LIMIT);
    assert_eq!(opened, snapshot(OPEN, OPEN, false));
    // Opposite of the interrupted direction. A waits out the reverse pause
    // (1s) instead of its 0s delay, B its regular 3s delay; each then only
    // runs the remaining distance plus overrun.
    assert_eq!(
        log.take(),
        [
            on(6000, A_OPEN),
            on(8000, B_OPEN),
            off(9000, A_OPEN),
            off(14000, B_OPEN),
        ]
    );
}

#[test]
fn impulse_after_stopped_opening_closes() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.impulse();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.impulse();
    clock.run_until_ready(step, IMMEDIATELY);
    log.take();

    controls.impulse();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(2)).is_none());
    assert_eq!(controls.cover_state(), CoverState::Closing);
    assert_eq!(log.take(), [on(6000, B_CLOSE)]);
}

#[test]
fn opposite_command_stops_then_reverses_after_pause() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.close();
    // Interrupted: both relays released right away...
    let interrupted = clock.run_until_ready(step, IMMEDIATELY);
    assert_eq!(interrupted, snapshot(85, 34, false));
    assert!(!log.any_high());
    log.take();

    // ...and the close runs next without another command, honoring the
    // reverse pause (B: 0s delay -> 1s pause) and A's 3s close delay.
    clock.run_until_ready(gate.step(), LIMIT);
    assert_eq!(
        log.take(),
        [
            on(6000, B_CLOSE),
            on(8000, A_CLOSE),
            off(9000, B_CLOSE),
            off(14000, A_CLOSE),
        ]
    );
}

#[test]
fn same_direction_command_does_not_interrupt() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.open();
    clock.run_until_ready(step, LIMIT);
    assert_eq!(log.take().len(), 4);
    assert_eq!(clock.now_ms(), 19000);
}

#[test]
fn stop_replaces_a_command_not_yet_picked_up() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    controls.stop();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);
}

#[test]
fn light_barrier_1_stops_any_movement_and_blocks_new_ones() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(4)).is_none());
    controls.barrier1_set(true);
    clock.run_until_ready(step, IMMEDIATELY);
    assert!(!log.any_high());
    log.take();

    // Blocked in both directions while the beam is broken.
    controls.open();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    controls.close();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);

    // Clearing the beam doesn't restart the gate by itself.
    controls.barrier1_set(false);
    assert!(
        clock
            .run_for(&mut Box::pin(gate.step()), Duration::from_secs(5))
            .is_none()
    );
    assert_eq!(log.take(), []);
}

#[test]
fn light_barrier_1_blocks_command_queued_before_it_broke() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.open();
    controls.barrier1_set(true);
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);
}

#[test]
fn light_barrier_2_stops_closing_only() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    // Opening ignores barrier 2.
    controls.barrier2_set(true);
    controls.open();
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(OPEN, OPEN, false)
    );
    log.take();

    // Closing is refused while it's broken...
    controls.close();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);

    // ...and stopped when it breaks mid-close.
    controls.barrier2_set(false);
    controls.close();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(4)).is_none());
    controls.barrier2_set(true);
    clock.run_until_ready(step, IMMEDIATELY);
    assert!(!log.any_high());
    assert_eq!(controls.cover_state(), CoverState::Open);
}

#[test]
fn wind_guard_reverses_closing_and_locks_until_reset() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.close();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.wind_trigger();
    clock.run_until_ready(step, IMMEDIATELY);
    // Reopens on its own (pending command), even though nobody pressed open.
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(OPEN, OPEN, true)
    );
    log.take();

    // Locked: normal commands are ignored.
    controls.close();
    controls.impulse();
    controls.stop();
    assert!(
        clock
            .run_for(&mut Box::pin(gate.step()), Duration::from_secs(5))
            .is_none()
    );
    assert_eq!(log.take(), []);

    // Reset clears the lock (and settles once to publish/persist it)...
    controls.reset_wind_lock();
    assert_eq!(
        clock.run_until_ready(gate.step(), IMMEDIATELY),
        snapshot(OPEN, OPEN, false)
    );
    // ...after which the gate can be closed again.
    controls.close();
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(CLOSED, CLOSED, false)
    );
}

#[test]
fn wind_lock_reset_during_forced_open_keeps_opening() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.wind_trigger();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.reset_wind_lock();

    // Same relay sequence as an uninterrupted open.
    assert_eq!(
        clock.run_until_ready(step, LIMIT),
        snapshot(OPEN, OPEN, false)
    );
    assert_eq!(
        log.take(),
        [
            on(0, A_OPEN),
            on(3000, B_OPEN),
            off(16000, A_OPEN),
            off(19000, B_OPEN),
        ]
    );
}

#[test]
fn wind_lock_reset_does_not_stop_closing() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.close();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    controls.reset_wind_lock();

    assert_eq!(
        clock.run_until_ready(step, LIMIT),
        snapshot(CLOSED, CLOSED, false)
    );
    assert_eq!(
        log.take(),
        [
            on(0, B_CLOSE),
            on(3000, A_CLOSE),
            off(16000, B_CLOSE),
            off(19000, A_CLOSE),
        ]
    );
}

static RESET_NOTIFICATIONS: AtomicUsize = AtomicUsize::new(0);

fn count_reset_notify() {
    RESET_NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn wind_lock_reset_mid_travel_is_announced_right_away() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let a = Motor::new(
        log.pin(A_OPEN),
        log.pin(A_CLOSE),
        LEAF_A,
        OVERRUN,
        REVERSE_PAUSE,
        CLOSED,
    );
    let b = Motor::new(
        log.pin(B_OPEN),
        log.pin(B_CLOSE),
        LEAF_B,
        OVERRUN,
        REVERSE_PAUSE,
        CLOSED,
    );
    let mut gate = Gate::new(&controls, a, b, false, count_reset_notify);

    controls.wind_trigger();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(5)).is_none());
    RESET_NOTIFICATIONS.store(0, Ordering::Relaxed);
    controls.reset_wind_lock();
    assert!(clock.run_for(&mut step, Duration::from_secs(1)).is_none());
    assert_eq!(RESET_NOTIFICATIONS.load(Ordering::Relaxed), 1);
    assert_eq!(controls.cover_state(), CoverState::Opening);
    assert!(!controls.wind_locked());
}

#[test]
fn wind_guard_open_waits_for_light_barrier_1_to_clear() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.barrier1_set(true);
    controls.wind_trigger();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);

    controls.barrier1_set(false);
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(OPEN, OPEN, true)
    );
}

#[test]
fn wind_lock_restored_at_boot_opens_without_a_command() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, true);

    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(OPEN, OPEN, true)
    );
    assert_eq!(log.take()[0], on(0, A_OPEN));
}

#[test]
fn barrier_bypass_ignores_both_barriers_for_one_movement() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.barrier1_set(true);
    controls.barrier2_set(true);
    controls.set_barrier_bypass(true);
    controls.close();
    assert_eq!(
        clock.run_until_ready(gate.step(), LIMIT),
        snapshot(CLOSED, CLOSED, false)
    );
    assert_eq!(log.take().len(), 4);
    // Used up by that movement: the barriers block again.
    assert!(!controls.barrier_bypass());
    controls.open();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);
}

#[test]
fn barrier_bypass_keeps_moving_when_a_barrier_breaks_mid_travel() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.set_barrier_bypass(true);
    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(4)).is_none());
    controls.barrier1_set(true);
    assert_eq!(
        clock.run_until_ready(step, LIMIT),
        snapshot(OPEN, OPEN, false)
    );
}

#[test]
fn disarming_barrier_bypass_mid_travel_stops_at_a_broken_barrier() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);

    controls.barrier1_set(true);
    controls.set_barrier_bypass(true);
    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(4)).is_none());
    controls.set_barrier_bypass(false);
    clock.run_until_ready(step, IMMEDIATELY);
    assert!(!log.any_high());
    assert_eq!(clock.now_ms(), 4000);
}

#[test]
fn barrier_bypass_is_kept_when_nothing_moved() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, OPEN, OPEN, false);

    controls.set_barrier_bypass(true);
    controls.open(); // already open
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    controls.stop();
    clock.run_until_ready(gate.step(), IMMEDIATELY);
    assert_eq!(log.take(), []);
    assert!(controls.barrier_bypass());
}

const LAMP: &str = "lamp";
const BLINK: Duration = Duration::from_millis(500);

#[test]
fn warning_light_blinks_only_while_moving() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let lamp_log = PinLog::new();
    let controls = Controls::new();
    let mut gate = gate(&controls, &log, CLOSED, CLOSED, false);
    let mut lamp = Box::pin(warning_light(&controls, lamp_log.pin(LAMP), BLINK));

    assert!(clock.run_for(&mut lamp, Duration::from_secs(2)).is_none());
    assert_eq!(lamp_log.take(), []);

    controls.open();
    let started = clock.now_ms();
    clock.run_until_ready(select(gate.step(), lamp.as_mut()), LIMIT);
    assert!(clock.run_for(&mut lamp, Duration::from_secs(5)).is_none());

    // Opening takes 19s: on/off every 500ms, then dark once settled.
    let expected: Vec<_> = (0..19)
        .flat_map(|second| {
            let at = started + second * 1000;
            [on(at, LAMP), off(at + 500, LAMP)]
        })
        .collect();
    assert_eq!(lamp_log.take(), expected);
    assert!(!lamp_log.is_high(LAMP));
}

#[test]
fn warning_light_blinks_while_barrier_bypass_is_armed() {
    let clock = sim::reset_clock();
    let lamp_log = PinLog::new();
    let controls = Controls::new();
    let mut lamp = Box::pin(warning_light(&controls, lamp_log.pin(LAMP), BLINK));

    controls.set_barrier_bypass(true);
    assert!(
        clock
            .run_for(&mut lamp, Duration::from_millis(1200))
            .is_none()
    );
    assert_eq!(
        lamp_log.take(),
        [on(0, LAMP), off(500, LAMP), on(1000, LAMP)]
    );

    // Goes dark at the latest after the current half period.
    controls.set_barrier_bypass(false);
    assert!(clock.run_for(&mut lamp, Duration::from_secs(3)).is_none());
    assert_eq!(lamp_log.take(), [off(1500, LAMP)]);
}

static NOTIFICATIONS: AtomicUsize = AtomicUsize::new(0);

fn count_notify() {
    NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn state_is_announced_at_boot_on_start_and_after_settling() {
    let clock = sim::reset_clock();
    let log = PinLog::new();
    let controls = Controls::new();
    NOTIFICATIONS.store(0, Ordering::Relaxed);
    let a = Motor::new(
        log.pin(A_OPEN),
        log.pin(A_CLOSE),
        LEAF_A,
        OVERRUN,
        REVERSE_PAUSE,
        CLOSED,
    );
    let b = Motor::new(
        log.pin(B_OPEN),
        log.pin(B_CLOSE),
        LEAF_B,
        OVERRUN,
        REVERSE_PAUSE,
        CLOSED,
    );
    let mut gate = Gate::new(&controls, a, b, false, count_notify);
    assert_eq!(NOTIFICATIONS.load(Ordering::Relaxed), 1);

    controls.open();
    let mut step = Box::pin(gate.step());
    assert!(clock.run_for(&mut step, Duration::from_secs(1)).is_none());
    assert_eq!(NOTIFICATIONS.load(Ordering::Relaxed), 2); // "opening"
    clock.run_until_ready(step, LIMIT);
    assert_eq!(NOTIFICATIONS.load(Ordering::Relaxed), 3); // "open"
}
