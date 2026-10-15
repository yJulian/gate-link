use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embedded_hal::digital::OutputPin;
use log::{info, warn};

use crate::motor::{Direction, Motor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateCommand {
    Open,
    Close,
    Stop,
    /// Push button / radio remote. Resolved by the gate itself (see
    /// [`Controls::resolve_impulse`]), since only it knows reliably whether
    /// the gate is moving right now.
    Impulse,
}

/// The gate as Home Assistant's `cover` sees it. A partially open gate
/// counts as open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverState {
    Open,
    Opening,
    Closed,
    Closing,
}

/// Leaf positions and wind lock after a command settled - what the firmware
/// persists to flash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Snapshot {
    pub leaf_a_position: u8,
    pub leaf_b_position: u8,
    pub wind_locked: bool,
}

const DIR_IDLE: u8 = 0;
const DIR_OPENING: u8 = 1;
const DIR_CLOSING: u8 = 2;

const POS_CLOSED: u8 = 0;
const POS_PARTIAL: u8 = 1;
const POS_OPEN: u8 = 2;

/// Everything shared between the gate task and the rest of the system: the
/// command slot, safety inputs and the published state. All methods take
/// `&self` and never block, so they can be called from any task (input
/// watchers, MQTT handler) - the firmware keeps one `static` instance.
pub struct Controls {
    /// The pending command. Latest command wins: sending never blocks or
    /// fails, and a newer command (e.g. STOP) replaces one the gate hasn't
    /// picked up yet. Safety conditions are re-checked by the gate itself
    /// when it acts, so they can't be lost here.
    command: Signal<CriticalSectionRawMutex, GateCommand>,
    /// Wakes a running movement whenever a light barrier changes, so it
    /// re-checks whether it's still allowed to move.
    barrier_changed: Signal<CriticalSectionRawMutex, ()>,
    /// What the gate is doing right now (including leaves still waiting out
    /// their initial delay). `DIR_IDLE` whenever no movement is in flight.
    current_direction: AtomicU8,
    /// The most recent non-idle direction, kept around after the motors go
    /// idle, so an impulse can continue in the opposite direction. Defaults
    /// to "closing" so the very first impulse at a partial position opens.
    last_direction: AtomicU8,
    /// Coarse classification of the leaf positions, refreshed after every
    /// settle: closed only when both leaves are fully closed, open only when
    /// both are fully open, partial otherwise.
    position: AtomicU8,
    /// Set by the wind guard, cleared by its reset input / MQTT button. While
    /// set, the gate is forced open and all normal open/close/stop/impulse
    /// inputs are ignored.
    wind_locked: AtomicBool,
    /// Light barrier 1: stops the gate regardless of direction, and blocks
    /// any movement while the beam stays broken.
    barrier1_active: AtomicBool,
    /// Light barrier 2: stops/blocks closing only, never opening.
    barrier2_active: AtomicBool,
    /// Both light barriers are ignored while set - a workaround for faulty
    /// barriers. One-shot: cleared again once the next movement ends.
    barrier_bypass: AtomicBool,
    /// Wakes the warning light whenever [`Controls::warning_light_active`]
    /// may have turned true.
    light_changed: Signal<CriticalSectionRawMutex, ()>,
}

impl Default for Controls {
    fn default() -> Self {
        Self::new()
    }
}

impl Controls {
    pub const fn new() -> Self {
        Controls {
            command: Signal::new(),
            barrier_changed: Signal::new(),
            current_direction: AtomicU8::new(DIR_IDLE),
            last_direction: AtomicU8::new(DIR_CLOSING),
            position: AtomicU8::new(POS_CLOSED),
            wind_locked: AtomicBool::new(false),
            barrier1_active: AtomicBool::new(false),
            barrier2_active: AtomicBool::new(false),
            barrier_bypass: AtomicBool::new(false),
            light_changed: Signal::new(),
        }
    }

    fn send(&self, command: GateCommand) {
        self.command.signal(command);
    }

    pub fn wind_locked(&self) -> bool {
        self.wind_locked.load(Ordering::Relaxed)
    }

    pub fn open(&self) {
        if self.wind_locked() {
            warn!("Gate locked by wind guard - ignoring open()");
            return;
        }
        self.send(GateCommand::Open);
    }

    pub fn close(&self) {
        if self.wind_locked() {
            warn!("Gate locked by wind guard - ignoring close()");
            return;
        }
        self.send(GateCommand::Close);
    }

    pub fn stop(&self) {
        if self.wind_locked() {
            warn!("Gate locked by wind guard - ignoring stop()");
            return;
        }
        self.send(GateCommand::Stop);
    }

    /// Local push button and radio remote: if the gate is moving, stop it.
    /// If it's idle and closed, open; idle and open, close. If it's idle
    /// mid-travel, go the opposite of the direction that was interrupted.
    pub fn impulse(&self) {
        if self.wind_locked() {
            warn!("Gate locked by wind guard - ignoring impulse");
            return;
        }
        self.send(GateCommand::Impulse);
    }

    /// Wind guard tripped: latches the lock and forces the gate open,
    /// bypassing the normal gating - only light barrier 1 can still hold it
    /// back (and the open is retried once barrier 1 clears).
    pub fn wind_trigger(&self) {
        self.wind_locked.store(true, Ordering::Relaxed);
        self.send(GateCommand::Open);
    }

    /// Clears the wind lock. Queues a (harmless if already idle) STOP so the
    /// gate republishes/persists the new state promptly.
    pub fn reset_wind_lock(&self) {
        self.wind_locked.store(false, Ordering::Relaxed);
        self.send(GateCommand::Stop);
    }

    /// Light barrier 1 (always stops, regardless of direction).
    pub fn barrier1_set(&self, active: bool) {
        self.barrier1_active.store(active, Ordering::Relaxed);
        self.barrier_changed.signal(());
        if !active && self.wind_locked() {
            // Keep pursuing "always open" once the obstruction clears.
            self.send(GateCommand::Open);
        }
    }

    /// Light barrier 2 (only stops closing; opening is never affected).
    pub fn barrier2_set(&self, active: bool) {
        self.barrier2_active.store(active, Ordering::Relaxed);
        self.barrier_changed.signal(());
    }

    /// Arms (or disarms) the light barrier bypass: the next open/close
    /// movement ignores both light barriers, and the bypass clears itself
    /// once that movement ends (completed or interrupted). Never moves the
    /// gate by itself.
    pub fn set_barrier_bypass(&self, active: bool) {
        self.barrier_bypass.store(active, Ordering::Relaxed);
        // A running movement re-checks whether it's still allowed to move.
        self.barrier_changed.signal(());
        self.light_changed.signal(());
    }

    pub fn barrier_bypass(&self) -> bool {
        self.barrier_bypass.load(Ordering::Relaxed)
    }

    /// The warning light blinks while the gate moves (including initial
    /// delays) and for as long as the light barrier bypass is armed.
    pub fn warning_light_active(&self) -> bool {
        self.current_direction.load(Ordering::Relaxed) != DIR_IDLE || self.barrier_bypass()
    }

    pub(crate) async fn warning_light_changed(&self) {
        self.light_changed.wait().await;
    }

    pub fn cover_state(&self) -> CoverState {
        match self.current_direction.load(Ordering::Relaxed) {
            DIR_OPENING => CoverState::Opening,
            DIR_CLOSING => CoverState::Closing,
            _ if self.position.load(Ordering::Relaxed) == POS_CLOSED => CoverState::Closed,
            _ => CoverState::Open,
        }
    }

    /// Why the gate may not move in `direction` right now, if anything.
    /// Checked by the gate before and continuously during every movement.
    fn blocked(&self, direction: Direction) -> Option<&'static str> {
        let barriers_checked = !self.barrier_bypass();
        if barriers_checked && self.barrier1_active.load(Ordering::Relaxed) {
            Some("light barrier 1 interrupted")
        } else if barriers_checked
            && direction == Direction::Closing
            && self.barrier2_active.load(Ordering::Relaxed)
        {
            Some("light barrier 2 interrupted")
        } else if direction == Direction::Closing && self.wind_locked() {
            Some("wind lock engaged")
        } else {
            None
        }
    }

    /// Turns an impulse into a concrete command while the gate is idle.
    fn resolve_impulse(&self, command: GateCommand) -> GateCommand {
        if command != GateCommand::Impulse {
            return command;
        }
        match self.position.load(Ordering::Relaxed) {
            POS_CLOSED => GateCommand::Open,
            POS_OPEN => GateCommand::Close,
            _ => match self.last_direction.load(Ordering::Relaxed) {
                DIR_OPENING => GateCommand::Close,
                _ => GateCommand::Open,
            },
        }
    }

    fn set_direction(&self, direction: Direction) {
        let value = match direction {
            Direction::Opening => DIR_OPENING,
            Direction::Closing => DIR_CLOSING,
        };
        self.current_direction.store(value, Ordering::Relaxed);
        self.last_direction.store(value, Ordering::Relaxed);
        self.light_changed.signal(());
    }
}

fn command_for(direction: Direction) -> GateCommand {
    match direction {
        Direction::Opening => GateCommand::Open,
        Direction::Closing => GateCommand::Close,
    }
}

/// Where one leaf is within the current movement.
#[derive(Clone, Copy)]
enum Phase {
    /// Waiting out its initial delay (and/or the reverse pause) until this instant.
    Waiting(Instant),
    /// Energized until this instant.
    Running(Instant),
    /// Finished, or never needed to move.
    Done,
}

/// The gate controller: the only owner of the two leaf motors, executing one
/// command at a time - "start, wait-or-get-interrupted, settle".
pub struct Gate<'c, P> {
    controls: &'c Controls,
    a: Motor<P>,
    b: Motor<P>,
    /// Called whenever [`Controls::cover_state`] may have changed (movement
    /// started, command settled) - the firmware publishes state from here.
    notify: fn(),
    /// A command that interrupted the previous movement, executed next.
    pending: Option<GateCommand>,
}

impl<'c, P: OutputPin<Error = Infallible>> Gate<'c, P> {
    /// Takes over both motors and publishes the restored state once. If the
    /// wind lock was still engaged (restored from flash), the first
    /// [`step`](Self::step) pursues "always open" right away instead of
    /// waiting for the anemometer to trip again.
    pub fn new(
        controls: &'c Controls,
        a: Motor<P>,
        b: Motor<P>,
        initial_wind_locked: bool,
        notify: fn(),
    ) -> Self {
        controls
            .wind_locked
            .store(initial_wind_locked, Ordering::Relaxed);
        let mut gate = Gate {
            controls,
            a,
            b,
            notify,
            pending: initial_wind_locked.then_some(GateCommand::Open),
        };
        gate.settle();
        gate
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            leaf_a_position: self.a.position(),
            leaf_b_position: self.b.position(),
            wind_locked: self.controls.wind_locked(),
        }
    }

    /// Waits for the next command (unless one is pending), executes it until
    /// it completes or is interrupted, and returns the settled state.
    pub async fn step(&mut self) -> Snapshot {
        let command = match self.pending.take() {
            Some(command) => command,
            None => self.controls.command.wait().await,
        };
        let command = self.controls.resolve_impulse(command);
        info!("Gate command: {command:?}");
        self.pending = match command {
            GateCommand::Open => self.drive(Direction::Opening).await,
            GateCommand::Close => self.drive(Direction::Closing).await,
            GateCommand::Stop | GateCommand::Impulse => {
                self.a.stop();
                self.b.stop();
                None
            }
        };
        self.settle()
    }

    /// Re-classifies the leaf positions after a command and notifies.
    fn settle(&mut self) -> Snapshot {
        let controls = self.controls;
        controls
            .current_direction
            .store(DIR_IDLE, Ordering::Relaxed);
        let snapshot = self.snapshot();
        let position = match (snapshot.leaf_a_position, snapshot.leaf_b_position) {
            (0, 0) => POS_CLOSED,
            (255, 255) => POS_OPEN,
            _ => POS_PARTIAL,
        };
        controls.position.store(position, Ordering::Relaxed);
        (self.notify)();
        snapshot
    }

    /// Moves both leaves in `direction`: each leaf waits its own initial
    /// delay, then runs until it reaches the end (plus overrun). Returns
    /// early - with both motors stopped - on a light barrier/wind lock block
    /// or on any command other than one for the same direction; a command
    /// for the opposite direction is handed back to be executed next. A
    /// movement that energized a motor uses up an armed barrier bypass.
    async fn drive(&mut self, direction: Direction) -> Option<GateCommand> {
        let controls = self.controls;
        if let Some(reason) = controls.blocked(direction) {
            warn!("Not moving: {reason}");
            return None;
        }
        controls.set_direction(direction);
        (self.notify)();

        let now = Instant::now();
        let a_phase = plan(&self.a, direction, now);
        let b_phase = plan(&self.b, direction, now);
        let mut leaves = [(&mut self.a, a_phase), (&mut self.b, b_phase)];
        let mut moved = false;

        let result = loop {
            if let Some(reason) = controls.blocked(direction) {
                warn!("Stopping: {reason}");
                break None;
            }

            let now = Instant::now();
            let mut next_deadline: Option<Instant> = None;
            for (motor, phase) in leaves.iter_mut() {
                if let Phase::Waiting(start_at) = *phase
                    && now >= start_at
                {
                    *phase = match motor.run_time(direction) {
                        Some(run_time) => {
                            motor.start(direction);
                            moved = true;
                            Phase::Running(now + run_time)
                        }
                        None => Phase::Done,
                    };
                }
                if let Phase::Running(stop_at) = *phase
                    && now >= stop_at
                {
                    motor.stop();
                    *phase = Phase::Done;
                }
                let deadline = match *phase {
                    Phase::Waiting(at) | Phase::Running(at) => at,
                    Phase::Done => continue,
                };
                next_deadline = Some(next_deadline.map_or(deadline, |next| next.min(deadline)));
            }

            let Some(next_deadline) = next_deadline else {
                break None;
            };

            match select3(
                Timer::at(next_deadline),
                controls.command.wait(),
                controls.barrier_changed.wait(),
            )
            .await
            {
                Either3::Second(command) if command == command_for(direction) => {}
                Either3::Second(GateCommand::Stop | GateCommand::Impulse) => break None,
                Either3::Second(command) => break Some(command),
                Either3::First(()) | Either3::Third(()) => {}
            }
        };

        for (motor, _) in leaves.iter_mut() {
            motor.stop();
        }
        if moved && controls.barrier_bypass.swap(false, Ordering::Relaxed) {
            info!("Light barrier bypass used up");
        }
        result
    }
}

fn plan<P: OutputPin<Error = Infallible>>(
    motor: &Motor<P>,
    direction: Direction,
    now: Instant,
) -> Phase {
    if motor.run_time(direction).is_none() {
        return Phase::Done;
    }
    Phase::Waiting((now + motor.initial_delay(direction)).max(motor.ready_at()))
}

#[cfg(test)]
mod tests;
