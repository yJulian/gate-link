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
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{AnyPin, Input, InputConfig, Level, Output, OutputConfig, Pin, Pull};
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::{Interface, WifiController};
use esp_storage::FlashStorage;
use log::{error, info};
use mqtt_gate::app::{discovery, mqtt_handler};
use mqtt_gate::infra::config::AppConfig;
use mqtt_gate::infra::storage::SharedFlash;
use mqtt_gate::infra::{
    dhcp_server, mqtt_client, provisioning_http, reset_button, storage, wifi_ap, wifi_sta,
};
use mqtt_gate::mk_static;
use mqtt_gate::physical::{self, gate_config};
use picoserve::AppBuilder as _;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    // Reboot rather than hang: a hung controller ignores the push button and
    // the light barriers until someone power-cycles it. The relays are driven
    // low again first thing on boot.
    esp_hal::system::software_reset()
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

fn relay_output(pin: AnyPin<'static>) -> Output<'static> {
    Output::new(pin, Level::Low, OutputConfig::default())
}

fn pulled_up_input(pin: AnyPin<'static>) -> Input<'static> {
    Input::new(pin, InputConfig::default().with_pull(Pull::Up))
}

/// Keeps a mode function alive (and with it whatever it owns, e.g. the Wi-Fi
/// controller) once all its work has been handed off to spawned tasks.
async fn idle_forever() -> ! {
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32s3 -o alloc -o unstable-hal -o wifi -o embassy -o ble-trouble -o log -o vscode -o zed -o ci

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // --- Gate wiring - change the GPIO numbers to match your setup. Timing
    // (run times, per-leaf initial delays) lives in `physical::gate_config`. ---
    //
    // ESP32-S3 pins that must stay free here: strapping pins (GPIO0/3/45/46),
    // USB D-/D+ (GPIO19/20), UART0 console (GPIO43/44), the module's
    // flash/PSRAM (GPIO26-37), the devkit's RGB LED (GPIO38/48), and ADC1
    // (GPIO1-10, minus the ones used as inputs below) for motor current
    // sensing later - ADC2 can't be used while Wi-Fi is on.
    //
    // Relay outputs come first, before anything else, so the motors are
    // guaranteed off as early as possible after reset.
    let leaf_a_open = relay_output(peripherals.GPIO15.degrade());
    let leaf_a_close = relay_output(peripherals.GPIO16.degrade());
    let leaf_b_open = relay_output(peripherals.GPIO17.degrade());
    let leaf_b_close = relay_output(peripherals.GPIO18.degrade());
    // LAMP_BLINK net: the warning light, driven like a relay.
    let warning_light = relay_output(peripherals.GPIO21.degrade());

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // `FlashStorage::new` panics if called more than once, so this is the one and
    // only instance for the whole program's lifetime, shared behind a mutex - the
    // gate task persists position/wind-lock state through it while provisioning
    // may concurrently need it to save the submitted config.
    let flash = mk_static!(
        SharedFlash,
        Mutex::new(FlashStorage::new(peripherals.FLASH))
    );

    // GPIO0 = onboard "BOOT" button on most ESP32 devkits. Held for 5s at boot,
    // this clears the stored config and drops the device back into provisioning mode.
    let mut boot_button = pulled_up_input(peripherals.GPIO0.degrade());
    reset_button::check_and_maybe_erase(&mut boot_button, &mut *flash.lock().await).await;

    let app_config = storage::load(&mut *flash.lock().await).await;
    let gate_state = storage::load_gate_state(&mut *flash.lock().await)
        .await
        .unwrap_or_default();

    // All inputs are active-low with an internal pull-up, same as the BOOT button.
    let push_button = pulled_up_input(peripherals.GPIO7.degrade());
    let radio_button = pulled_up_input(peripherals.GPIO8.degrade());
    let wind_sensor = pulled_up_input(peripherals.GPIO9.degrade());
    let wind_reset = pulled_up_input(peripherals.GPIO10.degrade());
    let light_barrier1 = pulled_up_input(peripherals.GPIO11.degrade());
    let light_barrier2 = pulled_up_input(peripherals.GPIO12.degrade());
    // IN7: arms the one-shot light barrier bypass (same as the MQTT switch).
    let barrier_bypass_button = pulled_up_input(peripherals.GPIO13.degrade());

    // Local gate control is started before - and independently of - any
    // networking: push button, radio remote, wind guard and light barriers
    // keep working without Wi-Fi/MQTT and even in provisioning mode.
    let leaf_a = physical::gate::MotorSettings {
        open_pin: leaf_a_open,
        close_pin: leaf_a_close,
        timing: gate_config::LEAF_A,
        initial_position: gate_state.leaf_a_position,
    };
    let leaf_b = physical::gate::MotorSettings {
        open_pin: leaf_b_open,
        close_pin: leaf_b_close,
        timing: gate_config::LEAF_B,
        initial_position: gate_state.leaf_b_position,
    };
    spawner.spawn(physical::gate::task(leaf_a, leaf_b, flash, gate_state.wind_locked).unwrap());
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
        physical::inputs::edge_task(barrier_bypass_button, physical::gate::arm_barrier_bypass)
            .unwrap(),
    );
    spawner.spawn(physical::gate::warning_light_task(warning_light).unwrap());

    let (wifi_controller, interfaces) =
        match esp_radio::wifi::new(peripherals.WIFI, Default::default()) {
            Ok(wifi) => wifi,
            Err(err) => {
                error!("Failed to initialize Wi-Fi controller: {err:?} - local control only");
                idle_forever().await
            }
        };

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    match app_config {
        Some(cfg) if !cfg.wifi_ssid.is_empty() => {
            run_station_mode(spawner, wifi_controller, interfaces.station, cfg, seed).await
        }
        _ => {
            run_provisioning_mode(
                spawner,
                wifi_controller,
                interfaces.access_point,
                flash,
                seed,
            )
            .await
        }
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "constructing StackResources<4> briefly puts it on the stack before mk_static! moves it into a StaticCell"
)]
async fn run_provisioning_mode(
    spawner: Spawner,
    mut controller: WifiController<'static>,
    interface: Interface<'static>,
    flash: &'static SharedFlash,
    seed: u64,
) -> ! {
    info!(
        "No Wi-Fi configured - starting provisioning hotspot {:?}",
        wifi_ap::AP_SSID
    );

    if let Err(err) = wifi_ap::configure(&mut controller) {
        error!("Failed to configure Wi-Fi access point: {err:?} - local control only");
        idle_forever().await;
    }

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

    match storage::save(&mut *flash.lock().await, &cfg).await {
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
async fn run_station_mode(
    spawner: Spawner,
    mut controller: WifiController<'static>,
    interface: Interface<'static>,
    cfg: AppConfig,
    seed: u64,
) -> ! {
    info!("Joining Wi-Fi network {:?}", cfg.wifi_ssid);

    if let Err(err) = wifi_sta::configure(&mut controller, &cfg) {
        error!("Failed to configure Wi-Fi station: {err:?} - local control only");
        idle_forever().await;
    }

    let (stack, runner) = embassy_net::new(
        interface,
        wifi_sta::net_config(),
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        seed,
    );

    // None of these block anything local: Wi-Fi (re)connects in the
    // background, and the MQTT client waits for the network on its own.
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(wifi_sta::connection_task(controller).unwrap());
    spawner.spawn(mqtt_client::task(stack, cfg).unwrap());
    spawner.spawn(mqtt_handler::task().unwrap());
    spawner.spawn(discovery::task().unwrap());

    idle_forever().await
}
