//! # Async MQTT Client for Embedded Systems (vendored, patched fork)
//!
//! `mqtt-async-embedded` is a `no_std` compatible, asynchronous MQTT v3.1.1 client
//! designed for embedded systems, built upon the [Embassy](https://embassy.dev/) async
//! ecosystem. See this crate's README.md for what was changed vs. the upstream
//! crates.io release and why.
//!
//! ## Usage
//!
//! To use the client, you need to provide a transport implementation, configure the client options,
//! and then run the `poll` method continuously to handle keep-alives and incoming messages.
//!
//! ```no_run
//! # use mqtt_async_embedded::client::{MqttClient, MqttOptions};
//! # use mqtt_async_embedded::packet::QoS;
//! # use mqtt_async_embedded::transport::MqttTransport;
//! #
//! # struct MyTransport;
//! # impl MqttTransport for MyTransport {
//! #     type Error = ();
//! #     async fn send(&mut self, buf: &[u8]) -> Result<(), Self::Error> { Ok(()) }
//! #     async fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> { Ok(0) }
//! # }
//! #
//! # async fn run() -> Result<(), mqtt_async_embedded::error::MqttError<()>> {
//! let transport = MyTransport;
//! let options = MqttOptions::new("my-device-id", "mqtt.broker.com", 1883)
//!     .with_credentials("user", b"pass");
//! let mut client = MqttClient::<_, 5, 256>::new(transport, options);
//!
//! client.connect().await?;
//! client.publish("sensors/temperature", b"25.3", QoS::AtLeastOnce).await?;
//!
//! loop {
//!     // Poll the client to process incoming messages and send keep-alives.
//!     if let Some(event) = client.poll().await? {
//!         // Handle incoming publish packets, ACKs, etc.
//!     }
//! }
//! # Ok(())
//! # }
//! ```

#![no_std]

pub mod client;
pub mod error;
pub mod packet;
pub mod transport;
pub mod util;

// Re-export key types for easier access at the crate root.
pub use client::{MqttClient, MqttOptions};
pub use packet::QoS;
