#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pin;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::Interface;
use esp_storage::FlashStorage;
use log::{error, info};
use mqtt_gate::app::{discovery, mqtt_handler};
use mqtt_gate::infra::config::AppConfig;
use mqtt_gate::infra::{
    dhcp_server, mqtt_client, provisioning_http, reset_button, storage, wifi_ap, wifi_sta,
};
use mqtt_gate::mk_static;
use mqtt_gate::physical;
use picoserve::AppBuilder as _;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

/// Drives the embassy-net stack for either the station or the access-point
/// interface (both are the same concrete `Interface` type from esp-radio).
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32 -o alloc -o unstable-hal -o wifi -o embassy -o ble-trouble -o log -o vscode -o zed -o ci

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // `FlashStorage::new` panics if called more than once, so this is promoted to
    // a `&'static mut` up front and threaded through as the one and only instance
    // for the whole program's lifetime - `storage::{load,save,erase,load_gate_state,
    // save_gate_state}` all take a `&mut FlashStorage` rather than constructing
    // their own, and the gate task holds onto it for the rest of station mode to
    // persist position/wind-lock state on every stop.
    let flash = mk_static!(FlashStorage<'static>, FlashStorage::new(peripherals.FLASH));

    let mut boot_button = esp_hal::gpio::Input::new(
        peripherals.GPIO0.degrade(),
        esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
    );

    // GPIO0 = onboard "BOOT" button on most ESP32 devkits. Held for 5s at boot,
    // this clears the stored config and drops the device back into provisioning mode.
    reset_button::check_and_maybe_erase(&mut boot_button, flash).await;

    // --- Gate wiring - change the GPIO numbers, durations and input polarity to
    // match your setup. All inputs below are active-low with an internal pull-up,
    // same as the BOOT button above. ---
    const LEFT_MOTOR_DURATION_SECS: u8 = 15;
    const RIGHT_MOTOR_DURATION_SECS: u8 = 15;

    fn pulled_up_input(pin: esp_hal::gpio::AnyPin<'static>) -> esp_hal::gpio::Input<'static> {
        esp_hal::gpio::Input::new(
            pin,
            esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
        )
    }

    let push_button = pulled_up_input(peripherals.GPIO4.degrade());
    let radio_button = pulled_up_input(peripherals.GPIO13.degrade());
    let wind_sensor = pulled_up_input(peripherals.GPIO16.degrade());
    let wind_reset = pulled_up_input(peripherals.GPIO17.degrade());
    let light_barrier1 = pulled_up_input(peripherals.GPIO18.degrade());
    let light_barrier2 = pulled_up_input(peripherals.GPIO19.degrade());

    let (mut wifi_controller, interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    match storage::load(flash).await {
        Some(cfg) if !cfg.wifi_ssid.is_empty() => {
            let gate_state = storage::load_gate_state(flash).await.unwrap_or_default();
            let left_motor_settings = physical::gate::MotorSettings {
                open_pin: peripherals.GPIO25.degrade(),
                close_pin: peripherals.GPIO26.degrade(),
                duration: LEFT_MOTOR_DURATION_SECS,
                initial_position: gate_state.left_position,
            };
            let right_motor_settings = physical::gate::MotorSettings {
                open_pin: peripherals.GPIO27.degrade(),
                close_pin: peripherals.GPIO14.degrade(),
                duration: RIGHT_MOTOR_DURATION_SECS,
                initial_position: gate_state.right_position,
            };
            run_station_mode(
                spawner,
                &mut wifi_controller,
                interfaces.station,
                cfg,
                seed,
                push_button,
                radio_button,
                wind_sensor,
                wind_reset,
                light_barrier1,
                light_barrier2,
                flash,
                gate_state.wind_locked,
                left_motor_settings,
                right_motor_settings,
            )
            .await;
        }
        _ => {
            run_provisioning_mode(
                spawner,
                &mut wifi_controller,
                interfaces.access_point,
                flash,
                seed,
            )
            .await;
        }
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "constructing StackResources<4> briefly puts it on the stack before mk_static! moves it into a StaticCell"
)]
async fn run_provisioning_mode(
    spawner: Spawner,
    controller: &mut esp_radio::wifi::WifiController<'static>,
    interface: Interface<'static>,
    flash: &mut FlashStorage<'static>,
    seed: u64,
) -> ! {
    info!(
        "No Wi-Fi configured - starting provisioning hotspot {:?}",
        wifi_ap::AP_SSID
    );

    wifi_ap::configure(controller).expect("Failed to configure Wi-Fi access point");

    let (stack, runner) = embassy_net::new(
        interface,
        wifi_ap::net_config(),
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );

    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(dhcp_server::task(stack).unwrap());

    let app = mk_static!(
        picoserve::AppRouter<provisioning_http::AppProps>,
        provisioning_http::AppProps.build_app()
    );
    let http_config = mk_static!(
        picoserve::Config,
        picoserve::Config::new(picoserve::Timeouts::default())
    );
    spawner.spawn(provisioning_http::task(0, stack, app, http_config).unwrap());

    let cfg = provisioning_http::CONFIG_SUBMITTED.wait().await;

    match storage::save(flash, &cfg).await {
        Ok(()) => info!("Config saved - rebooting into station mode"),
        Err(err) => error!("Failed to save config: {err:?}"),
    }

    // Give the "saved, rebooting" HTTP response time to flush before resetting.
    Timer::after(Duration::from_millis(500)).await;
    esp_hal::system::software_reset();
}

#[allow(
    clippy::large_stack_frames,
    reason = "constructing StackResources<4> briefly puts it on the stack before mk_static! moves it into a StaticCell"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct peripheral/config handle threaded through from main; grouping them would just add an intermediate struct with no behavior of its own"
)]
async fn run_station_mode(
    spawner: Spawner,
    controller: &mut esp_radio::wifi::WifiController<'static>,
    interface: Interface<'static>,
    cfg: AppConfig,
    seed: u64,
    push_button: esp_hal::gpio::Input<'static>,
    radio_button: esp_hal::gpio::Input<'static>,
    wind_sensor: esp_hal::gpio::Input<'static>,
    wind_reset: esp_hal::gpio::Input<'static>,
    light_barrier1: esp_hal::gpio::Input<'static>,
    light_barrier2: esp_hal::gpio::Input<'static>,
    flash: &'static mut FlashStorage<'static>,
    wind_locked: bool,
    left_motor_settings: physical::gate::MotorSettings,
    right_motor_settings: physical::gate::MotorSettings,
) -> ! {
    info!("Joining Wi-Fi network {:?}", cfg.wifi_ssid);

    wifi_sta::configure(controller, &cfg).expect("Failed to configure Wi-Fi station");
    controller
        .connect_async()
        .await
        .expect("Failed to connect to Wi-Fi");

    let (stack, runner) = embassy_net::new(
        interface,
        wifi_sta::net_config(),
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );

    spawner.spawn(net_task(runner).unwrap());

    info!("Waiting for network...");
    stack.wait_config_up().await;
    info!("Network is up");

    spawner.spawn(mqtt_client::task(stack, cfg).unwrap());
    spawner.spawn(mqtt_handler::task().unwrap());
    spawner.spawn(discovery::task().unwrap());
    spawner.spawn(physical::inputs::edge_task(push_button, physical::gate::impulse).unwrap());
    spawner.spawn(physical::inputs::edge_task(radio_button, physical::gate::impulse).unwrap());
    spawner.spawn(physical::inputs::edge_task(wind_sensor, physical::gate::wind_trigger).unwrap());
    spawner
        .spawn(physical::inputs::edge_task(wind_reset, physical::gate::reset_wind_lock).unwrap());
    spawner
        .spawn(physical::inputs::level_task(light_barrier1, physical::gate::barrier1_set).unwrap());
    spawner
        .spawn(physical::inputs::level_task(light_barrier2, physical::gate::barrier2_set).unwrap());
    spawner.spawn(
        physical::gate::task(
            left_motor_settings,
            right_motor_settings,
            flash,
            wind_locked,
        )
        .unwrap(),
    );

    loop {
        info!("Hello world!");
        Timer::after(Duration::from_secs(60)).await;
    }
}
