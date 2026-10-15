# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Firmware for an ESP32-S3 that drives a two-leaf sliding/swing gate (two 24V-relay
motors) and exposes it to Home Assistant over MQTT (with HA MQTT Discovery).
`no_std` Rust on Embassy (async executor via `esp-rtos`), targeting
`xtensa-esp32s3-none-elf`.

## Build / lint / flash

This is embedded firmware, not a hosted binary — the firmware itself needs
physical hardware (or `espflash`) to run. The hardware-independent gate logic
lives in the `gate-core` workspace crate and *is* unit-tested on the host.

```sh
cargo build --release              # what CI builds
cargo fmt --all -- --check         # CI also runs this
cargo clippy --all-features --workspace -- -D warnings   # CI: warnings are hard errors
cargo test-core                    # CI: host unit tests of gate-core and the MQTT client (alias, see below)
cargo run                          # builds + flashes + monitors via espflash (see .cargo/config.toml runner)
```

`cargo test-core` is an alias in `.cargo/config.toml`: plain `cargo test`
doesn't work because the default target is xtensa and the global `build-std`
can only be extended (not cleared) from the CLI, so the alias builds for
`host-tuple` and adds `std`/`test` to `build-std`. Tests run on a simulated
clock (embassy-time `mock-driver`) via the harness in `gate-core/src/sim.rs`:
mock relay pins log every switch with its millisecond timestamp, so tests
assert exact relay sequences without real waiting.

The toolchain is pinned via `rust-toolchain.toml` to the `esp` channel
(installed with [`espup`](https://github.com/esp-rs/espup)), and the target
(`xtensa-esp32s3-none-elf`) plus `build-std` are set in `.cargo/config.toml` —
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
- **`physical/`** — GPIO-level gate control: wiring the `gate-core` logic to
  real relay outputs, the timing constants (`gate_config.rs`), and generic
  debounced input watchers.

`src/lib.rs` just re-exports these three plus the `mk_static!` macro
(promotes a runtime value to `&'static mut` via a leaked `StaticCell` — the
standard way to give Embassy tasks something with `'static` lifetime).
`src/bin/main.rs` is the only place that owns peripherals, wires GPIO pins to
gate settings, and spawns every Embassy task.

### Boot flow (`main.rs`)

0. Very first thing after `esp_hal::init`: drive all four relay outputs low,
   so the motors are guaranteed off as early as possible after reset.
1. Init heap, timers, `esp-rtos`. Construct the *one* `FlashStorage` instance
   for the process lifetime (`esp_storage::FlashStorage::new` panics if
   called twice — it's wrapped in an async mutex (`infra::storage::SharedFlash`),
   promoted to `&'static` via `mk_static!` and shared by everything that
   touches flash).
2. Check the BOOT button (GPIO0): held 5s at boot → erase stored config →
   provisioning mode (`infra::reset_button`).
3. Load config + persisted gate state, then spawn the gate task and one
   `physical::inputs` task per GPIO input — **before and independent of any
   networking**. Local control (buttons, radio, wind guard, light barriers)
   must never depend on Wi-Fi/MQTT, and also runs in provisioning mode.
4. No config (or blank SSID) → `run_provisioning_mode`: bring up a Wi-Fi AP +
   DHCP server + `picoserve` HTTP form (`infra::provisioning_http`), wait for
   a submitted config, save it, then `esp_hal::system::software_reset()` to
   reboot into station mode.
5. Config present → `run_station_mode`: configure the station, bring up
   `embassy-net`, spawn `wifi_sta::connection_task` (connects and reconnects
   forever), the MQTT client task, the MQTT handler and HA discovery. Nothing
   here is awaited by local control; failures are logged, never `expect`ed.

GPIO pin numbers and input polarity are hardcoded in `main.rs`; gate timing
(per-leaf run times and initial delays, end-stop overrun, reverse pause)
lives in `physical::gate_config` as compile-time constants. The panic
handler reboots the chip rather than hanging.

### MQTT: transport vs. meaning

`infra::mqtt_client` owns the *only* broker connection and knows nothing
about topic semantics:
- `mqtt_client::publish(...)` — any task may call this directly to send. It is
  synchronous and never blocks (drops the oldest queued message when full),
  so a dead broker can't stall the caller. `mqtt_client::wait_connected()`
  fires after every (re)connect; `app::discovery::task` uses it to republish
  discovery configs and the current state.
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
  credentials, has a stub/no-op `publish`/`decode`, and didn't frame packets
  in the TCP stream. See that crate's README for the exact diff from upstream
  before touching MQTT wire-level code; its tests run with `cargo test-core`.

### Gate control (`gate-core` crate + `physical/gate.rs` glue)

All gate control logic lives in the `gate-core` workspace crate (no ESP32
dependencies, generic over `embedded_hal::digital::OutputPin`) so it can be
unit-tested on the host; **change gate behavior there and add a test in
`gate-core/src/gate/tests.rs`**. `src/physical/gate.rs` only holds the
firmware glue: the `static CONTROLS`, `fn()` wrappers for inputs/MQTT,
`publish_state` and the task that persists each settled `Snapshot` to flash.

- `gate_core::Controls` holds the shared state as atomics (current/last
  direction, coarse position, wind lock, two light-barrier flags) plus a
  latest-wins command `Signal` (sending never blocks or fails; a newer
  command replaces an unconsumed one). `gate_core::Gate::step` is driven from
  a single task loop — intentionally the only place that touches the two
  `Motor` instances, so there's one linear sequence of "start,
  wait-or-get-interrupted, settle" per command.
- `gate_core::Motor` tracks one leaf's position (0..255) as a
  time-based estimate (energize a relay, record `Instant`, derive travelled
  distance from elapsed time on `settle()`, separately for open/close run
  time) rather than reading a real position sensor — there is no feedback
  encoder. `END_STOP_OVERRUN` deliberately overruns the calculated target so
  the leaf reaches its physical end stop despite drift; `REVERSE_PAUSE`
  keeps a motor off for a moment before it may be energized again.
- `Gate::drive` moves both leaves in one direction: each leaf waits its own
  `open_initial_delay`/`close_initial_delay` (so e.g. A opens first, B closes
  first), then runs its remaining fraction of `open_duration`/`close_duration`
  plus overrun. Light barriers and the wind lock are re-checked by the task
  itself before and throughout every movement (`blocked`), not just when a
  command is sent.
- Public control entry points (`open`, `close`, `stop`, `impulse`,
  `wind_trigger`, `reset_wind_lock`, `barrier1_set`, `barrier2_set`) are
  called from both `app::mqtt_handler` and `physical::inputs` tasks (push
  button / radio remote / wind sensor / light barriers) — wind lock overrides
  everything except barrier 1; barrier 1 always stops; barrier 2 only blocks
  closing. `impulse` is resolved inside the gate task (moving → stop; closed
  → open; open → close; stopped mid-travel → opposite of last direction).
- `set_barrier_bypass` (MQTT switch, or a press on the IN7 input) arms a
  one-shot bypass: the next movement that energizes a motor ignores both
  light barriers, then the bypass clears itself. It is never persisted.
- `gate_core::warning_light` (own task, `LAMP_BLINK` = GPIO21) blinks while
  the gate moves (incl. initial delays) and as long as the bypass is armed.
- After every command settles (completes or is interrupted), position and
  wind-lock state are re-derived, published to MQTT (retained), and persisted
  to flash if changed (`infra::storage::{save_gate_state,load_gate_state}`) —
  so a power loss never loses track of leaf position or an engaged wind lock.

### Adding a new physical input

Use `physical::inputs::edge_task` (fires `action: fn()` once per press,
debounced: press and release must each be stable for 50ms — for buttons/pulses) or `level_task` (calls `setter: fn(bool)` on
every level change, no debounce — for continuously-monitored state like the
light barriers). Both are active-low with an internal pull-up by convention;
spawn them from `run_station_mode` in `main.rs` alongside the existing ones.

### Flash storage (`infra::storage`)

Two independent key-value entries (`AppConfig` and `GateState`) written into
the dedicated `app_cfg` data partition (see `partitions.csv`) via
`sequential-storage`'s log-structured map — chosen over raw flash writes so
this gets wear-leveling and power-loss safety for free. All storage functions
take `&mut FlashStorage` (obtained by locking the `SharedFlash` mutex) rather
than constructing their own, per the single-instance constraint noted above.

## Notes

- `#![no_std]` throughout; `alloc` is available (`extern crate alloc`) but
  there's no OS underneath — no threads, no filesystem, no host-side testing.
- `clippy::mem_forget` and `clippy::large_stack_frames` are hard denies at
  the crate root (`src/bin/main.rs`) — stack space and buffer lifetimes are
  scarce/safety-critical on this target. Local `#[allow(...)]`s in `main.rs`
  are intentional and explained inline (e.g. one-time large stack usage
  building `StackResources`), not something to silently propagate elsewhere.
