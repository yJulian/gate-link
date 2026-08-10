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

use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use mqtt_async_embedded::packet::QoS;

use crate::app::mqtt_topics::{OFF_PAYLOAD, ON_PAYLOAD, SPARE_INPUT_STATE_TOPIC};
use crate::infra::mqtt_client;

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

/// Not wired to any gate behavior - mirrors its level to MQTT 1:1 on every
/// change (retained), so it's inert unless something else in Home Assistant
/// is built on top of it. Unlike [`level_task`]'s `setter: fn(bool)`, the
/// side effect here is an async MQTT publish, so this doesn't fit that
/// generic shape and gets its own task.
#[embassy_executor::task]
pub async fn spare_input_task(pin: Input<'static>) -> ! {
    let mut last_active = pin.is_low();
    publish_spare_state(last_active).await;
    loop {
        let active = pin.is_low();
        if active != last_active {
            publish_spare_state(active).await;
            last_active = active;
        }
        Timer::after(LEVEL_POLL_INTERVAL).await;
    }
}

async fn publish_spare_state(active: bool) {
    let payload = if active { ON_PAYLOAD } else { OFF_PAYLOAD };
    mqtt_client::publish(
        SPARE_INPUT_STATE_TOPIC,
        payload.as_bytes().to_vec(),
        QoS::AtLeastOnce,
        true,
    )
    .await;
}
