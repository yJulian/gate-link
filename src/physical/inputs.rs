//! Generic GPIO input watchers wired to `crate::physical::gate`.
//!
//! Two shapes cover every physical input this device has:
//! - [`edge_task`]: active-low, debounced, fires `action` once per press.
//!   Used for the local push button, the radio remote, the wind guard pulse
//!   and its reset input - they're all "something happened, react once".
//! - [`level_task`]: active-low, no debounce delay, calls `setter` on every
//!   level change. Used for the two light barriers, which need to be tracked
//!   continuously (broken/clear) rather than just on the initial edge.

use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const DEBOUNCE: Duration = Duration::from_millis(200);
const LEVEL_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[embassy_executor::task(pool_size = 4)]
pub async fn edge_task(pin: Input<'static>, action: fn()) -> ! {
    let mut last_low = false;
    loop {
        let is_low = pin.is_low();
        if is_low && !last_low {
            action();
            Timer::after(DEBOUNCE).await;
        }
        last_low = is_low;
        Timer::after(POLL_INTERVAL).await;
    }
}

#[embassy_executor::task(pool_size = 2)]
pub async fn level_task(pin: Input<'static>, setter: fn(bool)) -> ! {
    let mut last_active = pin.is_low();
    setter(last_active);
    loop {
        let active = pin.is_low();
        if active != last_active {
            setter(active);
            last_active = active;
        }
        Timer::after(LEVEL_POLL_INTERVAL).await;
    }
}
