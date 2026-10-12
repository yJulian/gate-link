use core::convert::Infallible;

use embassy_time::{Duration, Instant};
use embedded_hal::digital::OutputPin;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Opening,
    Closing,
}

/// Per-leaf timing, see the firmware's `physical::gate_config` for the
/// installation's values.
#[derive(Clone, Copy, Debug)]
pub struct LeafTiming {
    /// Delay between an OPEN command and this leaf starting to open.
    pub open_initial_delay: Duration,
    /// Time this leaf needs to travel from fully closed to fully open.
    pub open_duration: Duration,
    /// Delay between a CLOSE command and this leaf starting to close.
    pub close_initial_delay: Duration,
    /// Time this leaf needs to travel from fully open to fully closed.
    pub close_duration: Duration,
}

/// Which relay is currently energized, and since when - used to derive the
/// up-to-date `position` on demand instead of polling a timer continuously.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Running(Direction, Instant),
}

/// One gate leaf driven by an open relay and a close relay. There is no
/// position sensor: the position is a time-based estimate derived from how
/// long each relay was energized.
///
/// * `position` = 0..255, 0 = closed, 255 = open
pub struct Motor<P> {
    open_pin: P,
    close_pin: P,
    position: u8,
    timing: LeafTiming,
    /// Extra run time added to every movement, so the leaf reliably reaches
    /// its physical end stop despite the position being only an estimate.
    end_stop_overrun: Duration,
    /// Minimum time the motor stays de-energized before it may be started
    /// again, to protect motor and relays from an instant reversal.
    reverse_pause: Duration,
    state: State,
    /// When the relays were last released after running, for `reverse_pause`.
    stopped_at: Option<Instant>,
}

impl<P: OutputPin<Error = Infallible>> Motor<P> {
    pub fn new(
        mut open_pin: P,
        mut close_pin: P,
        timing: LeafTiming,
        end_stop_overrun: Duration,
        reverse_pause: Duration,
        initial_position: u8,
    ) -> Self {
        let Ok(()) = open_pin.set_low();
        let Ok(()) = close_pin.set_low();
        Motor {
            open_pin,
            close_pin,
            position: initial_position,
            timing,
            end_stop_overrun,
            reverse_pause,
            state: State::Idle,
            stopped_at: None,
        }
    }

    /// Current position, 0..255 (0 = closed, 255 = open). Always up to date
    /// while idle; call after a command completes/is interrupted.
    pub fn position(&self) -> u8 {
        self.position
    }

    fn full_duration(&self, direction: Direction) -> Duration {
        match direction {
            Direction::Opening => self.timing.open_duration,
            Direction::Closing => self.timing.close_duration,
        }
    }

    /// Delay between a command in `direction` and this leaf starting to move.
    pub(crate) fn initial_delay(&self, direction: Direction) -> Duration {
        match direction {
            Direction::Opening => self.timing.open_initial_delay,
            Direction::Closing => self.timing.close_initial_delay,
        }
    }

    /// Earliest instant this motor may be energized again, honoring
    /// `reverse_pause` after it was last stopped.
    pub(crate) fn ready_at(&self) -> Instant {
        match self.stopped_at {
            Some(stopped_at) => stopped_at + self.reverse_pause,
            None => Instant::from_ticks(0),
        }
    }

    /// How long this leaf has to run to reach the end in `direction` from
    /// its current position, including the end-stop overrun. `None` if the
    /// leaf is already at that end and doesn't need to move at all.
    pub(crate) fn run_time(&self, direction: Direction) -> Option<Duration> {
        let remaining = match direction {
            Direction::Opening => 255 - self.position,
            Direction::Closing => self.position,
        };
        if remaining == 0 {
            return None;
        }
        let full = self.full_duration(direction).as_millis();
        Some(Duration::from_millis(full * remaining as u64 / 255) + self.end_stop_overrun)
    }

    /// Brings `position` up to date with whatever relay has been energized
    /// since the last state change, and releases both relays.
    fn settle(&mut self) {
        let State::Running(direction, start) = self.state else {
            return;
        };

        let Ok(()) = self.open_pin.set_low();
        let Ok(()) = self.close_pin.set_low();
        let now = Instant::now();
        self.state = State::Idle;
        self.stopped_at = Some(now);

        let full = self.full_duration(direction).as_millis().max(1);
        let travelled = ((now - start).as_millis() * 255 / full).min(255) as u8;
        self.position = match direction {
            Direction::Opening => self.position.saturating_add(travelled),
            Direction::Closing => self.position.saturating_sub(travelled),
        };
    }

    pub(crate) fn start(&mut self, direction: Direction) {
        self.settle();
        // Release the opposite relay before energizing this one.
        let Ok(()) = match direction {
            Direction::Opening => self
                .close_pin
                .set_low()
                .and_then(|()| self.open_pin.set_high()),
            Direction::Closing => self
                .open_pin
                .set_low()
                .and_then(|()| self.close_pin.set_high()),
        };
        self.state = State::Running(direction, Instant::now());
    }

    pub(crate) fn stop(&mut self) {
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{self, MockPin, PinLog};

    const TIMING: LeafTiming = LeafTiming {
        open_initial_delay: Duration::from_secs(0),
        open_duration: Duration::from_secs(10),
        close_initial_delay: Duration::from_secs(0),
        close_duration: Duration::from_secs(20),
    };

    fn motor(log: &PinLog, position: u8) -> Motor<MockPin> {
        Motor::new(
            log.pin("open"),
            log.pin("close"),
            TIMING,
            Duration::from_secs(1),
            Duration::from_millis(500),
            position,
        )
    }

    #[test]
    fn full_run_includes_overrun_from_both_end_positions() {
        let _clock = sim::reset_clock();
        let log = PinLog::new();
        // Regression: the old u8 math saturated, so a full run from an end
        // position got no overrun at all.
        assert_eq!(
            motor(&log, 0).run_time(Direction::Opening),
            Some(Duration::from_secs(11))
        );
        assert_eq!(
            motor(&log, 255).run_time(Direction::Closing),
            Some(Duration::from_secs(21))
        );
    }

    #[test]
    fn already_at_end_does_not_move() {
        let _clock = sim::reset_clock();
        let log = PinLog::new();
        assert_eq!(motor(&log, 255).run_time(Direction::Opening), None);
        assert_eq!(motor(&log, 0).run_time(Direction::Closing), None);
    }

    #[test]
    fn partial_run_uses_remaining_fraction_of_direction_specific_duration() {
        let _clock = sim::reset_clock();
        let log = PinLog::new();
        let m = motor(&log, 51); // 20% open
        // 80% of 10s open + 1s overrun, 20% of 20s close + 1s overrun.
        assert_eq!(
            m.run_time(Direction::Opening),
            Some(Duration::from_millis(9000))
        );
        assert_eq!(
            m.run_time(Direction::Closing),
            Some(Duration::from_millis(5000))
        );
    }

    #[test]
    fn position_tracks_elapsed_time_per_direction() {
        let clock = sim::reset_clock();
        let log = PinLog::new();
        let mut m = motor(&log, 0);

        m.start(Direction::Opening);
        clock.advance(Duration::from_secs(5)); // half of the 10s open time
        m.stop();
        assert_eq!(m.position(), 127);

        m.start(Direction::Closing);
        clock.advance(Duration::from_secs(5)); // a quarter of the 20s close time
        m.stop();
        assert_eq!(m.position(), 64);
    }

    #[test]
    fn position_saturates_at_the_ends() {
        let clock = sim::reset_clock();
        let log = PinLog::new();
        let mut m = motor(&log, 200);
        m.start(Direction::Opening);
        clock.advance(Duration::from_secs(30));
        m.stop();
        assert_eq!(m.position(), 255);
    }

    #[test]
    fn relays_are_exclusive_and_released_on_stop() {
        let _clock = sim::reset_clock();
        let log = PinLog::new();
        let mut m = motor(&log, 100);

        m.start(Direction::Opening);
        assert!(log.is_high("open") && !log.is_high("close"));
        m.start(Direction::Closing);
        assert!(!log.is_high("open") && log.is_high("close"));
        m.stop();
        assert!(!log.is_high("open") && !log.is_high("close"));
    }

    #[test]
    fn reverse_pause_starts_when_the_motor_stops() {
        let clock = sim::reset_clock();
        let log = PinLog::new();
        let mut m = motor(&log, 100);
        assert_eq!(m.ready_at(), Instant::from_ticks(0));

        m.start(Direction::Opening);
        clock.advance(Duration::from_secs(2));
        m.stop();
        assert_eq!(m.ready_at(), Instant::now() + Duration::from_millis(500));
    }
}
