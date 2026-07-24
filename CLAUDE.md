# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Firmware for an ESP32 that drives a two-leaf sliding/swing gate (two 24V-relay
motors) and exposes it to Home Assistant over MQTT (with HA MQTT Discovery).
`no_std` Rust on Embassy (async executor via `esp-rtos`), targeting
`xtensa-esp32-none-elf`.

## Build / lint / flash

This is embedded firmware, not a hosted binary — there is no `cargo test` /
`cargo run` on the host; `cargo build`/`clippy`/`fmt` are the things that
actually work locally, and physical hardware (or `espflash`) is needed to run it.

```sh
cargo build --release              # what CI builds
cargo fmt --all -- --check         # CI also runs this
cargo clippy --all-features --workspace -- -D warnings   # CI: warnings are hard errors
cargo run                          # builds + flashes + monitors via espflash (see .cargo/config.toml runner)
```

The toolchain is pinned via `rust-toolchain.toml` to the `esp` channel
(installed with [`espup`](https://github.com/esp-rs/espup)), and the target
(`xtensa-esp32-none-elf`) plus `build-std` are set in `.cargo/config.toml` —
`cargo build` with no `--target` already does the right thing. CI
(`.github/workflows/rust_ci.yml`) runs build/fmt/clippy on `esp-rs/xtensa-toolchain`
for every push to `main` and every PR.

## Architecture

Three-layer split, enforced by module docs (`src/app/mod.rs`,
`src/infra/mod.rs`) — keep new code on the correct side of it:

- **`infra/`** — talks to the outside world, knows nothing about gate
  semantics: Wi-Fi (AP + station), the provisioning HTTP server, DHCP server
  (AP mode only), flash storage, and the MQTT transport.
- **`app/`** — what the device does with the MQTT connection `infra` provides:
  the central inbound message handler and HA discovery config publishing.
- **`physical/`** — GPIO-level gate control: motor timing/position tracking
  and generic debounced input watchers.

`src/lib.rs` just re-exports these three plus the `mk_static!` macro
(promotes a runtime value to `&'static mut` via a leaked `StaticCell` — the
standard way to give Embassy tasks something with `'static` lifetime).
`src/bin/main.rs` is the only place that owns peripherals, wires GPIO pins to
gate settings, and spawns every Embassy task.

### Boot flow (`main.rs`)

1. Init heap, timers, `esp-rtos`. Construct the *one* `FlashStorage` instance
   for the process lifetime (`esp_storage::FlashStorage::new` panics if
   called twice — it's promoted to `&'static mut` via `mk_static!` and
   threaded through everywhere flash is touched).
2. Check the BOOT button (GPIO0): held 5s at boot → erase stored config →
   provisioning mode (`infra::reset_button`).
3. Try `infra::storage::load`. No config (or blank SSID) → `run_provisioning_mode`:
   bring up a Wi-Fi AP + DHCP server + `picoserve` HTTP form
   (`infra::provisioning_http`), wait for a submitted config, save it, then
   `esp_hal::system::software_reset()` to reboot into station mode.
4. Config present → `run_station_mode`: join Wi-Fi station, bring up
   `embassy-net`, spawn the MQTT client task, the MQTT handler, HA discovery,
   one `physical::inputs` task per GPIO input, and the gate task. All GPIO
   pin numbers, motor travel durations, and input polarity for the specific
   installation are hardcoded here — this is what to edit when the physical
   wiring changes.

### MQTT: transport vs. meaning

`infra::mqtt_client` owns the *only* broker connection and knows nothing
about topic semantics:
- `mqtt_client::publish(...)` — any task may call this directly to send.
- `mqtt_client::receive()` — deliberately drained by exactly one task,
  `app::mqtt_handler::task`, which is the single place inbound messages are
  interpreted (see the `match` in `on_message`). If you're adding a new
  inbound command, it goes there, not in a new consumer of `receive()`.
- `mqtt_client::subscribe(...)` re-subscribes automatically on reconnect
  (subscriptions are tracked in a static list and replayed against the new
  connection).
- Topic/payload string constants live centrally in `app::mqtt_topics`.
- The MQTT client itself is a vendored, patched fork
  (`vendor/mqtt-async-embedded`) — crates.io's 1.0.0 can't send CONNECT
  credentials and has a stub/no-op `publish`/`decode`. See that crate's
  README for the exact diff from upstream before touching MQTT wire-level code.

### Gate control (`physical/gate.rs` + `physical/motor.rs`)

- `gate.rs` is pure control logic operating on atomics (`CURRENT_DIRECTION`,
  `LAST_DIRECTION`, `POSITION`, `WIND_LOCKED`, two light-barrier flags) plus a
  depth-1 command channel, all driven from a single task loop
  (`gate::task`) — this is intentionally the only place that touches the two
  `Motor` instances, so there's one linear sequence of "start, wait-or-get-interrupted,
  settle" per command.
- `motor.rs` (`Motor`) tracks one leaf's position (0..255) as a
  time-based estimate (energize a relay, record `Instant`, derive travelled
  distance from elapsed time on `settle()`) rather than reading a real
  position sensor — there is no feedback encoder. `SLACK` deliberately
  overruns the calculated target so the leaf reaches its physical end stop
  despite drift.
- Two leaves are coordinated so they arrive together: `run_open` staggers the
  faster leaf's start so both finish at once; `run_close` starts both
  together and lets the faster one finish (and stop) early.
- Public control entry points (`open`, `close`, `stop`, `impulse`,
  `wind_trigger`, `reset_wind_lock`, `barrier1_set`, `barrier2_set`) are
  called from both `app::mqtt_handler` and `physical::inputs` tasks (push
  button / radio remote / wind sensor / light barriers) — they're the shared
  gating logic (wind lock overrides everything except barrier 1; barrier 1
  always stops; barrier 2 only blocks closing).
- After every command settles (completes or is interrupted), position and
  wind-lock state are re-derived, published to MQTT (retained), and persisted
  to flash (`infra::storage::{save_gate_state,load_gate_state}`) — so a power
  loss never loses track of leaf position or an engaged wind lock.

### Adding a new physical input

Use `physical::inputs::edge_task` (fires `action: fn()` once per press,
debounced — for buttons/pulses) or `level_task` (calls `setter: fn(bool)` on
every level change, no debounce — for continuously-monitored state like the
light barriers). Both are active-low with an internal pull-up by convention;
spawn them from `run_station_mode` in `main.rs` alongside the existing ones.

### Flash storage (`infra::storage`)

Two independent key-value entries (`AppConfig` and `GateState`) written into
the dedicated `app_cfg` data partition (see `partitions.csv`) via
`sequential-storage`'s log-structured map — chosen over raw flash writes so
this gets wear-leveling and power-loss safety for free. All storage functions
take `&mut FlashStorage` rather than constructing their own, per the
single-instance constraint noted above.

## Notes

- `#![no_std]` throughout; `alloc` is available (`extern crate alloc`) but
  there's no OS underneath — no threads, no filesystem, no host-side testing.
- `clippy::mem_forget` and `clippy::large_stack_frames` are hard denies at
  the crate root (`src/bin/main.rs`) — stack space and buffer lifetimes are
  scarce/safety-critical on this target. Local `#[allow(...)]`s in `main.rs`
  are intentional and explained inline (e.g. one-time large stack usage
  building `StackResources`), not something to silently propagate elsewhere.
