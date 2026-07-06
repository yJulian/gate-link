use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::AnyPin;
use log::warn;

use crate::physical::motor::Motor;

#[derive(Clone, Copy)]
pub(crate) enum GateCommand {
    OPEN, CLOSE, STOP
}

pub struct MotorSettings {
    pub open_pin: AnyPin<'static>,
    pub close_pin: AnyPin<'static>,
    pub duration: u8    // Seconds needed to travel the full 0..255 range
}

static COMMANDS: Channel<CriticalSectionRawMutex, GateCommand, 1> = Channel::new();

/// Intent behind the last `open`/`close` call, used by [`toggle`] to decide
/// which way to command the gate next. Not a substitute for the real,
/// motor-tracked position.
static LAST_OPEN: AtomicBool = AtomicBool::new(false);

fn send(command: GateCommand) {
    if COMMANDS.try_send(command).is_err() {
        warn!("Gate command dropped: a command is already pending");
    }
}

pub(crate) fn open() {
    LAST_OPEN.store(true, Ordering::Relaxed);
    send(GateCommand::OPEN);
}

pub(crate) fn close() {
    LAST_OPEN.store(false, Ordering::Relaxed);
    send(GateCommand::CLOSE);
}

pub(crate) fn stop() {
    send(GateCommand::STOP);
}

/// Convenience for a single physical/test button: opens if the gate was last
/// told to close, closes otherwise.
pub(crate) fn toggle() {
    if LAST_OPEN.load(Ordering::Relaxed) {
        close();
    } else {
        open();
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
        (left, right, left_remaining, right_remaining - left_remaining)
    } else {
        (right, left, right_remaining, left_remaining - right_remaining)
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
pub async fn task(left_settings: MotorSettings, right_settings: MotorSettings) {
    let mut left = Motor::new(left_settings);
    let mut right = Motor::new(right_settings);

    let mut pending = None;
    loop {
        let command = match pending.take() {
            Some(command) => command,
            None => COMMANDS.receive().await,
        };
        pending = match command {
            GateCommand::OPEN => run_open(&mut left, &mut right).await,
            GateCommand::CLOSE => run_close(&mut left, &mut right).await,
            GateCommand::STOP => {
                left.stop();
                right.stop();
                None
            }
        };
    }
}
