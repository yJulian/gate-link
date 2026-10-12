//! Generic GPIO input watchers wired to `crate::physical::gate`.
//!
//! Three shapes cover every physical input this device has:
//! - [`edge_task`]: active-low, debounced, fires `action` once per press.
//!   Used for the local push button, the radio remote, the wind guard pulse
//!   and its reset input - they're all "something happened, react once".
//! - [`level_task`]: active-low, no debounce delay, calls `setter` on every
//!   level change. Used for the two light barriers, which need to be tracked
//!   continuously (broken/clear) rather than just on the initial edge.
//! - [`spare_input_task`]: like `level_task`, but publishes straight to MQTT
//!   instead of calling into `crate::physical::gate` - for the one input on
//!   this board revision with no gate behavior assigned to it.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use mqtt_async_embedded::packet::QoS;

use crate::app::mqtt_topics::{OFF_PAYLOAD, ON_PAYLOAD, SPARE_INPUT_STATE_TOPIC};
use crate::infra::mqtt_client;

const EDGE_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Consecutive identical samples needed to accept a press or a release
/// (5 x 10ms = 50ms), so short spikes on long cables - e.g. induced by the
/// motors switching - don't count as a press.
const EDGE_STABLE_SAMPLES: u8 = 5;
const LEVEL_POLL_INTERVAL: Duration = Duration::from_millis(20);

static SPARE_INPUT_ACTIVE: AtomicBool = AtomicBool::new(false);

#[embassy_executor::task(pool_size = 4)]
pub async fn edge_task(pin: Input<'static>, action: fn()) -> ! {
    // true = released, waiting for a press; false = pressed, waiting for release.
    let mut armed = true;
    let mut stable = 0u8;
    loop {
        // While armed we're looking for low (press), otherwise for high (release).
        if pin.is_low() == armed {
            stable += 1;
            if stable >= EDGE_STABLE_SAMPLES {
                stable = 0;
                if armed {
                    action();
                }
                armed = !armed;
            }
        } else {
            stable = 0;
        }
        Timer::after(EDGE_POLL_INTERVAL).await;
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

/// Not wired to any gate behavior - mirrors its level to MQTT 1:1 on every
/// change (retained), so it's inert unless something else in Home Assistant
/// is built on top of it.
#[embassy_executor::task]
pub async fn spare_input_task(pin: Input<'static>) -> ! {
    let mut last_active = pin.is_low();
    SPARE_INPUT_ACTIVE.store(last_active, Ordering::Relaxed);
    publish_spare_state();
    loop {
        let active = pin.is_low();
        if active != last_active {
            SPARE_INPUT_ACTIVE.store(active, Ordering::Relaxed);
            publish_spare_state();
            last_active = active;
        }
        Timer::after(LEVEL_POLL_INTERVAL).await;
    }
}

/// Publishes the spare input's current level (retained, non-blocking).
pub fn publish_spare_state() {
    let payload = if SPARE_INPUT_ACTIVE.load(Ordering::Relaxed) {
        ON_PAYLOAD
    } else {
        OFF_PAYLOAD
    };
    mqtt_client::publish(
        SPARE_INPUT_STATE_TOPIC,
        payload.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    );
}
