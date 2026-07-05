//! Hold-to-reset: holding the boot button low for `HOLD_MS` at startup erases the
//! stored config, dropping the device back into provisioning (AP) mode on this same
//! boot.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{Input, InputConfig, InputPin, Pull};
use esp_storage::FlashStorage;

/// How long the button must be held continuously to trigger an erase.
const HOLD_MS: u64 = 5000;
const POLL_INTERVAL_MS: u64 = 50;

/// Samples `pin` (active-low, e.g. the onboard BOOT button on most ESP32 devkits) and,
/// if held continuously for [`HOLD_MS`], erases the stored config via `crate::storage::erase`.
///
/// Call this once at the very start of `main()`, before any Wi-Fi/network init, so an
/// erase-then-reboot-into-AP-mode happens within the same boot cycle.
pub async fn check_and_maybe_erase(pin: impl InputPin, flash: &mut FlashStorage<'_>) {
    let button = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
    let start = Instant::now();

    while button.is_low() {
        if Instant::now() - start >= Duration::from_millis(HOLD_MS) {
            log::warn!("Reset button held {HOLD_MS}ms - erasing stored config");
            if let Err(err) = crate::storage::erase(flash).await {
                log::error!("Failed to erase stored config: {err:?}");
            }
            return;
        }
        Timer::after(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}
