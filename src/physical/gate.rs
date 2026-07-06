use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Receiver;
use esp_hal::gpio::AnyPin;
use crate::physical::motor::Motor;

#[derive(Clone, Copy)]
pub(crate) enum GateCommand {
    OPEN,CLOSE,STOP
}

pub(crate) struct MotorSettings {
    pub control_pin: AnyPin<'static>,
    pub duration: u8    // Time from 0..255, 0 = closed, 255 = open
}

#[embassy_executor::task]
pub(crate) async fn task(left_settings: MotorSettings, right_settings: MotorSettings, rx: Receiver<'static, NoopRawMutex, GateCommand, 1>) {
    let left_motor: Motor = Motor::new(left_settings);
    let right_motor: Motor = Motor::new(right_settings);

    loop {
        let command = rx.receive().await;
        match command {
            GateCommand::OPEN => {
                left_motor.open();
                right_motor.open();
            },
            GateCommand::CLOSE => {
                left_motor.close();
                right_motor.close();
            },
            GateCommand::STOP => {
                left_motor.stop();
                right_motor.stop();
            }
        }
    }
}

