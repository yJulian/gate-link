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
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::Interface;
use esp_storage::FlashStorage;
use esp_hal::gpio::Pin;
use log::{error, info};
use mqtt_gate::app::{discovery, mqtt_handler};
use mqtt_gate::physical;
use mqtt_gate::infra::config::AppConfig;
use mqtt_gate::infra::{
    dhcp_server, mqtt_client, provisioning_http, reset_button, storage, wifi_ap, wifi_sta,
};
use mqtt_gate::mk_static;
use picoserve::AppBuilder as _;
use trouble_host::prelude::*;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

extern crate alloc;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

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
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // `FlashStorage::new` panics if called more than once, so this is the one and
    // only instance for the whole program's lifetime; `storage::{load,save,erase}`
    // all take a `&mut FlashStorage` rather than constructing their own.
    let mut flash = FlashStorage::new(peripherals.FLASH);

    let mut boot_button = esp_hal::gpio::Input::new(
        peripherals.GPIO0.degrade(),
        esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
    );

    // GPIO0 = onboard "BOOT" button on most ESP32 devkits. Held for 5s at boot,
    // this clears the stored config and drops the device back into provisioning mode.
    reset_button::check_and_maybe_erase(&mut boot_button, &mut flash).await;

    let (mut wifi_controller, interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default())
            .expect("Failed to initialize Wi-Fi controller");
    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    //let transport = BleConnector::new(peripherals.BT, Default::default()).unwrap();
    //let ble_controller = ExternalController::<_, 1>::new(transport);
    //let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
    //    HostResources::new();
    //let _stack = trouble_host::new(ble_controller, &mut resources);

    let rng = esp_hal::rng::Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    match storage::load(&mut flash).await {
        Some(cfg) if !cfg.wifi_ssid.is_empty() => {
            run_station_mode(
                spawner,
                &mut wifi_controller,
                interfaces.station,
                cfg,
                seed,
                boot_button,
            )
            .await;
        }
        _ => {
            run_provisioning_mode(
                spawner,
                &mut wifi_controller,
                interfaces.access_point,
                &mut flash,
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
async fn run_station_mode(
    spawner: Spawner,
    controller: &mut esp_radio::wifi::WifiController<'static>,
    interface: Interface<'static>,
    cfg: AppConfig,
    seed: u64,
    boot_button: esp_hal::gpio::Input<'static>,
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
    spawner.spawn(physical::button::task(boot_button).unwrap());

    loop {
        info!("Hello world!");
        Timer::after(Duration::from_secs(60)).await;
    }
}