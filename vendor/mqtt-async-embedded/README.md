# mqtt-async-embedded (vendored fork)

Vendored copy of [`mqtt-async-embedded` 1.0.0](https://crates.io/crates/mqtt-async-embedded), patched for use in `mqtt_gate` because the published crate:

- has no way to send a username/password in the CONNECT packet at all, and
- has non-functional `publish()`/`Publish::encode` (stub, sends nothing) and `Publish::decode` (stub, always empty).

Changes made in this fork:

- `MqttOptions::with_credentials(username, password)` — sets the CONNECT username/password flags and payload fields (MQTT 3.1.1 §3.1.3).
- Real `Publish` encode/decode (QoS 0 and 1, topic + optional packet id + payload).
- `MqttClient::publish()` now actually sends the packet instead of being a no-op.
- Fixed the CONNECT protocol name/level to real MQTT v3.1.1 (`"MQTT"`, level `4`) — the original hardcoded the older, mostly-obsolete v3.1 (`"MQIsdp"`, level `3`).
- Removed the incomplete/unused `v5` feature (its property parsing was a placeholder that didn't actually decode anything) and the `transport-smoltcp` feature (pinned to `embassy-net 0.7`, incompatible with this project's `0.9.1`) — `mqtt_gate` provides its own `MqttTransport` impl over its own `embassy-net` TCP socket instead.
- Dropped unused dependencies (`async-trait`, `futures`, `embedded-hal*`, `tokio`, `nom`, etc.) that weren't actually referenced by any code path still in this fork.

Not a general-purpose replacement for the upstream crate — trimmed to exactly what `mqtt_gate` needs.
