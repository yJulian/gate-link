# mqtt_gate

ESP32 firmware for a two-leaf gate controller (sliding/swing gate driven by
two 24V relay-controlled motors), exposed to [Home Assistant](https://www.home-assistant.io/)
over MQTT with [MQTT Discovery](https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery) —
no manual YAML entity setup needed.

`no_std` Rust on the [Embassy](https://embassy.dev/) async executor (via
`esp-rtos`), targeting `xtensa-esp32-none-elf`.

## Features

- Open / close / stop, plus impulse control (local push button and radio
  remote) with the classic open → stop → close → stop → open press cycle
- Two independent light barriers: one stops the gate outright, the other
  only blocks closing
- Wind guard (anemometer) input that force-opens and locks the gate until
  explicitly reset (dedicated input or an MQTT button)
- Gate leaf position and wind-lock state persisted to flash, so a power
  loss doesn't lose track of where the gate was
- First-boot provisioning: the device opens its own Wi-Fi hotspot with an
  HTTP form to collect Wi-Fi + MQTT settings — no hardcoded credentials

## Hardware / wiring

All GPIO assignments, motor travel durations and input polarity are
hardcoded in `src/bin/main.rs` — edit `run_station_mode`'s wiring section to
match your installation. Default pinout:

| Signal                       | GPIO |
|-------------------------------|------|
| BOOT button (config reset)    | 0    |
| Local push button             | 4    |
| Radio remote                  | 13   |
| Wind sensor (anemometer)      | 16   |
| Wind lock reset               | 17   |
| Light barrier 1                | 18   |
| Light barrier 2                | 19   |
| Left leaf: open relay          | 25   |
| Left leaf: close relay         | 26   |
| Right leaf: open relay         | 27   |
| Right leaf: close relay        | 14   |

All inputs are active-low with an internal pull-up. Leaf travel time
defaults to 15s per motor (`LEFT_/RIGHT_MOTOR_DURATION_SECS` in `main.rs`).

## Building

The toolchain is pinned via `rust-toolchain.toml` to the `esp` channel,
installed with [`espup`](https://github.com/esp-rs/espup). The target
(`xtensa-esp32-none-elf`) and `build-std` are already configured in
`.cargo/config.toml`, so no extra flags are needed:

```sh
cargo build --release              # what CI builds
cargo fmt --all -- --check
cargo clippy --all-features --workspace -- -D warnings
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
configs for three entities:

| Entity                          | Type          | Notes                                   |
|----------------------------------|---------------|------------------------------------------|
| Gate (`mqtt_gate_cover`)         | Cover         | open / close / stop, reports position    |
| Windwächter (`mqtt_gate_wind`)   | Binary sensor | on while the wind lock is engaged        |
| Windwächter zurücksetzen (`mqtt_gate_reset_anemometer`) | Button | clears the wind lock |

No manual entity configuration in Home Assistant is required — they appear
automatically once the device connects.
