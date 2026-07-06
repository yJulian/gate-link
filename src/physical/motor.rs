use esp_hal::gpio::AnyPin;
use esp_hal::xtensa_lx::interrupt::set;
use log::error;
use crate::physical::gate::MotorSettings;

///
/// * `position` = 0..255, 0 = closed, 255 = open`
/// * ``speed` = position / seconds
pub struct Motor<'a> {
    pin: AnyPin<'a>,
    position: u8,
    speed: u8
}

impl<'a> Motor<'a> {
    pub(crate) fn new(settings: MotorSettings) -> Self {
        if (settings.duration <= 0) {
            error!("Invalid duration");
        }
        Motor {
            pin: settings.control_pin,
            speed: 255/settings.duration,
            position: 0,
        }
    }

    pub(crate) fn open(&self) {

    }

    pub fn close(&self) {

    }

    pub fn stop(&self) {

    }

}