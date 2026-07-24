use crate::physical::gate::MotorSettings;
use embassy_time::{Duration, Instant};
use esp_hal::gpio::{Level, Output, OutputConfig};
use log::error;

/// Which relay is currently energized, and since when - used to derive the
/// up-to-date `position` on demand instead of polling a timer continuously.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Opening(Instant),
    Closing(Instant),
}

/// How much a start command overshoots its target end (0 or 255) by, applied
/// to the position used for the run-time calculation only. Compensates for
/// mechanical backlash/drift so the leaf reliably reaches the physical end
/// stop instead of stopping just short of it.
const SLACK: u8 = 20;

///
/// * `position` = 0..255, 0 = closed, 255 = open
/// * `duration` = time needed to travel the full 0..255 range
pub struct Motor<'a> {
    open_pin: Output<'a>,
    close_pin: Output<'a>,
    position: u8,
    duration: Duration,
    state: State,
}

impl<'a> Motor<'a> {
    pub(crate) fn new(settings: MotorSettings) -> Self {
        if settings.duration == 0 {
            error!("Invalid duration");
        }
        Motor {
            open_pin: Output::new(settings.open_pin, Level::Low, OutputConfig::default()),
            close_pin: Output::new(settings.close_pin, Level::Low, OutputConfig::default()),
            position: settings.initial_position,
            duration: Duration::from_secs(settings.duration.max(1) as u64),
            state: State::Idle,
        }
    }

    /// Current position, 0..255 (0 = closed, 255 = open). Always up to date
    /// while idle; call after a command completes/is interrupted.
    pub(crate) fn position(&self) -> u8 {
        self.position
    }

    /// Brings `position` up to date with whatever relay has been energized
    /// since the last state change, and releases both relays.
    fn settle(&mut self) {
        let (start, opening) = match self.state {
            State::Idle => return,
            State::Opening(start) => (start, true),
            State::Closing(start) => (start, false),
        };

        let elapsed = Instant::now() - start;
        let travelled = ((elapsed.as_millis() * 255) / self.duration.as_millis()).min(255) as u8;
        self.position = if opening {
            self.position.saturating_add(travelled)
        } else {
            self.position.saturating_sub(travelled)
        };

        self.open_pin.set_low();
        self.close_pin.set_low();
        self.state = State::Idle;
    }

    /// Time still needed to reach fully open (255) from the current position,
    /// biased by [`SLACK`] so the leaf overruns the calculated target a bit.
    pub(crate) fn remaining_open(&self) -> Duration {
        let biased = self.position.saturating_sub(SLACK);
        Duration::from_millis(self.duration.as_millis() * (255 - biased) as u64 / 255)
    }

    /// Time still needed to reach fully closed (0) from the current position,
    /// biased by [`SLACK`] so the leaf overruns the calculated target a bit.
    pub(crate) fn remaining_close(&self) -> Duration {
        let biased = self.position.saturating_add(SLACK);
        Duration::from_millis(self.duration.as_millis() * biased as u64 / 255)
    }

    pub(crate) fn start_open(&mut self) {
        self.settle();
        if self.position == 255 {
            return;
        }
        self.open_pin.set_high();
        self.close_pin.set_low();
        self.state = State::Opening(Instant::now());
    }

    pub(crate) fn start_close(&mut self) {
        self.settle();
        if self.position == 0 {
            return;
        }
        self.open_pin.set_low();
        self.close_pin.set_high();
        self.state = State::Closing(Instant::now());
    }

    pub(crate) fn stop(&mut self) {
        self.settle();
    }
}
