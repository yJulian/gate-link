# mqtt-async-embedded (vendored fork)

Vendored copy of [`mqtt-async-embedded` 1.0.0](https://crates.io/crates/mqtt-async-embedded), patched for use in `mqtt_gate` because the published crate:

- has no way to send a username/password in the CONNECT packet at all, and
- has non-functional `publish()`/`Publish::encode` (stub, sends nothing) and `Publish::decode` (stub, always empty).

Changes made in this fork:

- `MqttOptions::with_credentials(username, password)` — sets the CONNECT username/password flags and payload fields (MQTT 3.1.1 §3.1.3).
- Real `Publish` encode/decode (QoS 0 and 1, topic + optional packet id + payload, retain flag).
- `MqttClient::publish()` now actually sends the packet instead of being a no-op, and takes a `retain` flag (needed for retained Home Assistant MQTT discovery configs).
- Fixed the CONNECT protocol name/level to real MQTT v3.1.1 (`"MQTT"`, level `4`) — the original hardcoded the older, mostly-obsolete v3.1 (`"MQIsdp"`, level `3`).
- Removed the incomplete/unused `v5` feature (its property parsing was a placeholder that didn't actually decode anything) and the `transport-smoltcp` feature (pinned to `embassy-net 0.7`, incompatible with this project's `0.9.1`) — `mqtt_gate` provides its own `MqttTransport` impl over its own `embassy-net` TCP socket instead.
- Dropped unused dependencies (`async-trait`, `futures`, `embedded-hal*`, `tokio`, `nom`, etc.) that weren't actually referenced by any code path still in this fork.
- Packet framing on receive: TCP is a byte stream, but `poll()`/`connect()` used to decode each `recv()` result as exactly one packet. A packet split across reads was decoded truncated, and of several packets in one read only the first was seen (a PUBLISH's payload even swallowed the following packets). The client now buffers until a complete packet (by its remaining length) is available and hands out one packet per `poll()`. `poll()` stays cancel-safe.
- `poll()` acknowledges incoming QoS 1 PUBLISHes with a PUBACK, and skips packet types it doesn't use instead of failing the connection.
- `SubAck::decode` read the remaining length at the wrong offset; `ConnAck::decode` panicked on a short packet.
- Host-side tests (`src/tests.rs`, run by the workspace's `cargo test-core`) check every packet byte for byte against MQTT 3.1.1.

Not a general-purpose replacement for the upstream crate — trimmed to exactly what `mqtt_gate` needs.
