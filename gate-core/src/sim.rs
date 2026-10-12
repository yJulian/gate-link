//! Host-side test harness: a manually advanced clock, relay pins that log
//! every switch with its (simulated) timestamp, and a tiny executor that
//! polls a future while stepping the clock forward.

use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use embassy_time::{Duration, Instant, MockDriver};
use embedded_hal::digital::{ErrorType, OutputPin};

/// Simulation resolution: the clock advances in steps of this size, and the
/// future under test is polled after every step.
const STEP: Duration = Duration::from_millis(1);

/// The mock clock is a process-wide global, so tests using it must not run
/// concurrently. Holding a [`Clock`] serializes them.
static CLOCK_LOCK: Mutex<()> = Mutex::new(());

pub struct Clock {
    _guard: MutexGuard<'static, ()>,
}

/// Takes exclusive ownership of the mock clock and resets it to t = 0.
pub fn reset_clock() -> Clock {
    let guard = CLOCK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    MockDriver::get().reset();
    Clock { _guard: guard }
}

impl Clock {
    pub fn advance(&self, duration: Duration) {
        MockDriver::get().advance(duration);
    }

    pub fn now_ms(&self) -> u64 {
        Instant::now().as_millis()
    }

    /// Polls `future` at the current instant, then keeps stepping the clock
    /// and polling for `duration`. Returns the output if it completed;
    /// otherwise the future can be resumed later (pass a `Box::pin(..)`).
    pub fn run_for<F: Future + Unpin>(
        &self,
        future: &mut F,
        duration: Duration,
    ) -> Option<F::Output> {
        let end = Instant::now() + duration;
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(output) = Pin::new(&mut *future).poll(&mut cx) {
                return Some(output);
            }
            if Instant::now() >= end {
                return None;
            }
            self.advance(STEP);
        }
    }

    /// Like [`run_for`](Self::run_for), but takes the future by value and it
    /// must complete within `limit`.
    pub fn run_until_ready<F: Future>(&self, future: F, limit: Duration) -> F::Output {
        let mut future = pin!(future);
        self.run_for(&mut future, limit)
            .unwrap_or_else(|| panic!("future did not complete within {limit:?}"))
    }
}

/// One relay switching on or off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Switch {
    pub at_ms: u64,
    pub pin: &'static str,
    pub on: bool,
}

/// Shared log of all relay switches, plus their current levels.
#[derive(Clone, Default)]
pub struct PinLog(Rc<RefCell<PinLogInner>>);

#[derive(Default)]
struct PinLogInner {
    levels: HashMap<&'static str, bool>,
    switches: Vec<Switch>,
}

impl PinLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pin(&self, name: &'static str) -> MockPin {
        MockPin {
            name,
            log: self.clone(),
        }
    }

    pub fn is_high(&self, name: &'static str) -> bool {
        self.0.borrow().levels.get(name).copied().unwrap_or(false)
    }

    /// Any relay currently energized?
    pub fn any_high(&self) -> bool {
        self.0.borrow().levels.values().any(|&high| high)
    }

    /// Every level change since the last call, in order.
    pub fn take(&self) -> Vec<Switch> {
        core::mem::take(&mut self.0.borrow_mut().switches)
    }

    fn set(&self, pin: &'static str, on: bool) {
        let mut inner = self.0.borrow_mut();
        let previous = inner.levels.insert(pin, on).unwrap_or(false);
        if previous != on {
            inner.switches.push(Switch {
                at_ms: Instant::now().as_millis(),
                pin,
                on,
            });
        }
    }
}

pub struct MockPin {
    name: &'static str,
    log: PinLog,
}

impl ErrorType for MockPin {
    type Error = Infallible;
}

impl OutputPin for MockPin {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.log.set(self.name, false);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Infallible> {
        self.log.set(self.name, true);
        Ok(())
    }
}

/// Shorthand for building expected switch lists.
pub fn on(at_ms: u64, pin: &'static str) -> Switch {
    Switch {
        at_ms,
        pin,
        on: true,
    }
}

pub fn off(at_ms: u64, pin: &'static str) -> Switch {
    Switch {
        at_ms,
        pin,
        on: false,
    }
}
