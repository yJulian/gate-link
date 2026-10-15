//! Installation-specific gate timing. Everything here is a compile-time
//! constant - edit and rebuild to tune the gate to the physical leaves.
//!
//! On every OPEN/CLOSE command each leaf independently waits its
//! `*_initial_delay`, then runs for (at most) its `*_duration` plus
//! [`END_STOP_OVERRUN`]. A leaf starting from somewhere mid-travel only runs
//! for the remaining fraction of its duration (plus the overrun), but still
//! waits its full initial delay, so the order the leaves move in is always
//! preserved - which matters for overlapping leaves that must close in a
//! fixed order.
//!
//! Which GPIO pins drive leaf A and leaf B is set in `src/bin/main.rs`; the
//! logic using these values lives (unit-tested) in the `gate-core` crate.

use embassy_time::Duration;
use gate_core::LeafTiming;

/// Leaf A: opens first, closes second.
pub const LEAF_A: LeafTiming = LeafTiming {
    open_initial_delay: Duration::from_secs(0),
    open_duration: Duration::from_secs(15),
    close_initial_delay: Duration::from_secs(3),
    close_duration: Duration::from_secs(15),
};

/// Leaf B: opens second, closes first.
pub const LEAF_B: LeafTiming = LeafTiming {
    open_initial_delay: Duration::from_secs(3),
    open_duration: Duration::from_secs(15),
    close_initial_delay: Duration::from_secs(0),
    close_duration: Duration::from_secs(15),
};

/// Extra run time added to every movement, so a leaf reliably reaches its
/// physical end stop despite the position being only a time-based estimate.
pub const END_STOP_OVERRUN: Duration = Duration::from_secs(1);

/// Minimum time a motor stays de-energized before it may be started again
/// (in particular in the opposite direction), to protect motors and relays
/// from an instant reversal under load.
pub const REVERSE_PAUSE: Duration = Duration::from_millis(1000);

/// The warning light is on for this long, then off for this long, while
/// blinking.
pub const WARNING_LIGHT_HALF_PERIOD: Duration = Duration::from_millis(500);
