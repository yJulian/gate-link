# Gate Link

ESP32-S3 firmware for a two-leaf gate controller, exposed to [Home Assistant](https://www.home-assistant.io/) over MQTT with [MQTT Discovery](https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery). No manual YAML entity setup needed.

`no_std` Rust on the [Embassy](https://embassy.dev/) async executor (via `esp-rtos`), targeting `xtensa-esp32s3-none-elf`.

## Features

- Open / close / stop, plus impulse control (local push button and radio
  remote) with the classic open → stop → close → stop → open press cycle
- Two independent light barriers: one stops the gate outright, the other
  only blocks closing
- Wind guard (anemometer) input that force-opens and locks the gate until
  explicitly reset (dedicated input or an MQTT button)
- Light barrier bypass (MQTT switch or local input) for faulty barriers:
  the next open/close movement ignores both barriers, then it turns itself
  off again
- Warning light that blinks while the gate moves and while the bypass is on
- Gate leaf position and wind-lock state persisted to flash, so a power
  loss doesn't lose track of where the gate was
- First-boot provisioning: the device opens its own Wi-Fi hotspot with an
  HTTP form to collect Wi-Fi + MQTT settings — no hardcoded credentials

## Hardware / wiring

GPIO assignments and input polarity are hardcoded in the wiring section at
the top of `main()` in `src/bin/main.rs`; leaf timing lives in
`src/physical/gate_config.rs`. Default pinout (ESP32-S3-DevKitC-1):

| Signal                        | GPIO |
|-------------------------------|------|
| BOOT button (config reset)    | 0    |
| Leaf A: open / close relay    | 15 / 16 |
| Leaf B: open / close relay    | 17 / 18 |
| Warning light (`LAMP_BLINK`)  | 21   |
| IN1 local push button         | 7    |
| IN2 radio remote              | 8    |
| IN3 wind sensor (anemometer)  | 9    |
| IN4 wind lock reset           | 10   |
| IN5 light barrier 1           | 11   |
| IN6 light barrier 2           | 12   |
| IN7 light barrier bypass      | 13   |

All inputs are active-low with an internal pull-up. Each leaf runs 15s per
direction by default, with leaf A opening first and leaf B closing first.

## Building

The toolchain is pinned via `rust-toolchain.toml` to the `esp` channel,
installed with [`espup`](https://github.com/esp-rs/espup). The target
(`xtensa-esp32s3-none-elf`) and `build-std` are already configured in
`.cargo/config.toml`, so no extra flags are needed:

```sh
cargo build --release              # what CI builds
cargo fmt --all -- --check
cargo clippy --all-features --workspace -- -D warnings
cargo test-core                    # host unit tests (gate-core, app-core, MQTT client)
cargo run                          # builds, flashes and opens a serial monitor via espflash
```

## First boot / provisioning

1. On first boot (or after a config reset), the device has no stored Wi-Fi
   credentials and opens an open (passwordless) hotspot named
   `mqtt-gate-setup` at `192.168.2.1`.
2. Connect to it and open `http://192.168.2.1/` to fill in the Wi-Fi SSID/
   password and MQTT broker host/port/credentials.
3. Submitting the form persists the config to flash and reboots the device
   into station mode, where it joins your Wi-Fi and connects to the broker.
4. To reprovision, hold the onboard **BOOT** button (GPIO0) for 5 seconds
   during boot — this erases the stored config and drops back into
   provisioning mode.

## Home Assistant integration

On connecting to the broker, the device publishes retained MQTT Discovery
configs for four entities:

| Entity                                               | Type          | Notes                                              |
|------------------------------------------------------|---------------|----------------------------------------------------|
| Gate (`mqtt_gate_cover`)                             | Cover         | open / close / stop, reports position              |
| Wind Monitor Input (`mqtt_gate_wind`)                | Binary sensor | on while the wind lock is engaged                  |
| Light Barrier Bypass (`mqtt_gate_barrier_bypass`)    | Switch        | ignore the light barriers for the next movement    |
| Reset Windmonitor (`mqtt_gate_reset_anemometer`)     | Button        | clears the wind lock                               |

No manual entity configuration in Home Assistant is required — they appear
automatically once the device connects.
