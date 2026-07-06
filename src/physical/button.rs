use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use log::info;
use crate::infra::mqtt_client;
use crate::app::mqtt_topics::CONTACT_STATE_TOPIC;
use mqtt_async_embedded::packet::QoS;

use crate::physical::gate::toggle;

#[embassy_executor::task]
pub async fn task(button: Input<'static>) -> ! {
    let mut last_state = true; // High when unpressed
    let mut contact_sensor_state = false;

    loop {
        let is_low = button.is_low();
        if is_low && last_state {
            // Button transition from High to Low (pressed)
            info!("BOOT button pressed!");
            
            // Toggle contact sensor state
            contact_sensor_state = !contact_sensor_state;
            let payload = if contact_sensor_state { "ON" } else { "OFF" };
            info!("Publishing contact state: {}", payload);
            
            toggle();
            mqtt_client::publish(
                CONTACT_STATE_TOPIC,
                payload.as_bytes().to_vec(),
                QoS::AtLeastOnce,
                false,
            )
            .await;
            
            // Debounce
            Timer::after(Duration::from_millis(200)).await;
        }
        last_state = !is_low;
        Timer::after(Duration::from_millis(50)).await;
    }
}
