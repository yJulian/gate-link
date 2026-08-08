# GateLink PCB

KiCad hardware design for **GateLink Master**, the carrier board that hosts
an ESP32-DevKitC module and drives the two-leaf gate's four relays. This is
the board the [firmware](../README.md) runs on.

## What's on the board

- **U3** — ESP32-DevKitC module (socketed, not soldered directly)
- **K2–K5** — four SPDT relays (SANYOU SRD series, Form C), each switched
  through a transistor driver (Q2–Q5) with a flyback diode
- **J1** — 5-pin screw terminal for digital inputs (push button, radio
  remote, wind sensor, light barriers — see the firmware's input wiring)
- **J2–J5** — 3-pin screw terminals, one per relay (NO / COM / NC)
- **J6** — 2-pin screw terminal for 24V power input

Four relays map to the firmware's two motors: one open + one close relay
per gate leaf.

## Files

- `GateLinkMaster.kicad_pro` / `.kicad_sch` / `.kicad_pcb` — the KiCad
  project, schematic and board layout
- `fp-lib-table` / `sym-lib-table` — project-local footprint/symbol library
  tables (relative paths, so the project is self-contained if opened from
  this folder)
- `GateLinkMaster-backups/` — KiCad's automatic timestamped backups, kept
  out of the repo's diff history but versioned as zip snapshots
- `LICENSE` — GPLv3, applies to the hardware design files in this folder

## Opening the project

Requires [KiCad](https://www.kicad.org/) (developed against KiCad 9/10).
Open `GateLinkMaster.kicad_pro`; schematic and PCB editors will pick up the
project-local library tables automatically.
