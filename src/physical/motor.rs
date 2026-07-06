use esp_hal::gpio::{Level, Output, OutputConfig};
use embassy_time::{Duration, Instant};
use log::error;
use crate::physical::gate::MotorSettings;

/// Which relay is currently energized, and since when - used to derive the
/// up-to-date `position` on demand instead of polling a timer continuously.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Opening(Instant),
    Closing(Instant),
}

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
            position: 0,
            duration: Duration::from_secs(settings.duration.max(1) as u64),
            state: State::Idle,
        }
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

    /// Time still needed to reach fully open (255) from the current position.
    pub(crate) fn remaining_open(&self) -> Duration {
        Duration::from_millis(self.duration.as_millis() * (255 - self.position) as u64 / 255)
    }

    /// Time still needed to reach fully closed (0) from the current position.
    pub(crate) fn remaining_close(&self) -> Duration {
        Duration::from_millis(self.duration.as_millis() * self.position as u64 / 255)
    }

    pub(crate) fn start_open(&mut self) {
        self.settle();
        if self.position == 255 {
            return;
        }
        self.open_pin.set_high();
        self.state = State::Opening(Instant::now());
    }

    pub(crate) fn start_close(&mut self) {
        self.settle();
        if self.position == 0 {
            return;
        }
        self.close_pin.set_high();
        self.state = State::Closing(Instant::now());
    }

    pub(crate) fn stop(&mut self) {
        self.settle();
    }
}
