use core::convert::Infallible;

use embassy_time::{Duration, Timer};
use embedded_hal::digital::OutputPin;

use crate::gate::Controls;

/// Drives the warning light forever: blinks (`half_period` on, `half_period`
/// off, starting with "on") while [`Controls::warning_light_active`], and
/// keeps it off otherwise. Runs as its own task, independent of the gate
/// task, so blinking never delays relay switching.
pub async fn warning_light<P: OutputPin<Error = Infallible>>(
    controls: &Controls,
    mut pin: P,
    half_period: Duration,
) -> ! {
    let mut lit = false;
    loop {
        if controls.warning_light_active() {
            lit = !lit;
            let _ = pin.set_state(lit.into());
            Timer::after(half_period).await;
        } else {
            lit = false;
            let _ = pin.set_low();
            controls.warning_light_changed().await;
        }
    }
}
