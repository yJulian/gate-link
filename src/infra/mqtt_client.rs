//! Connects to the MQTT broker from the persisted `AppConfig` (host/port and,
//! optionally, username/password) and keeps the connection alive.
//!
//! This module only owns the transport: the CONNECT/keepalive handshake, and
//! shuttling bytes to and from the broker. It deliberately knows nothing about
//! *what* the messages mean:
//!
//! - Incoming `PUBLISH` packets are copied into an [`IncomingMessage`] and pushed
//!   onto a queue that [`receive`] drains. `crate::app::mqtt_handler` is the single
//!   place that reads from it and reacts.
//! - Outgoing messages go through [`publish`], which just enqueues them. Any
//!   module — now or in the future — can call it without needing access to the
//!   socket, which stays owned by this task alone.

use alloc::string::String;
use alloc::vec::Vec;
use core::net::IpAddr;
use core::cell::RefCell;

use embassy_futures::select::{select, Either};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use mqtt_async_embedded::client::{MqttClient, MqttEvent, MqttOptions};
use mqtt_async_embedded::packet::QoS;
use mqtt_async_embedded::transport::MqttTransport;

use crate::infra::config::AppConfig;

const MAX_TOPICS: usize = 4;
const MQTT_BUF_SIZE: usize = 512;
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// How many messages may be queued in either direction before the producer
/// (the broker for incoming, an app-side caller for outgoing) has to wait.
const QUEUE_DEPTH: usize = 8;

/// An MQTT message received from the broker, decoupled from the connection's
/// internal receive buffer so it can be handed off across a queue.
#[derive(Debug)]
pub struct IncomingMessage {
    pub topic: String,
    pub payload: Vec<u8>,
}

/// An MQTT message queued for publishing by [`publish`].
#[derive(Debug)]
struct OutgoingMessage {
    topic: String,
    payload: Vec<u8>,
    qos: QoS,
    retain: bool,
}

static INCOMING: Channel<CriticalSectionRawMutex, IncomingMessage, QUEUE_DEPTH> = Channel::new();
static OUTGOING: Channel<CriticalSectionRawMutex, OutgoingMessage, QUEUE_DEPTH> = Channel::new();

#[derive(Clone, Debug)]
pub struct Subscription {
    pub topic: String,
    pub qos: QoS,
}

static SUBSCRIPTIONS: embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, RefCell<Vec<Subscription>>> =
    embassy_sync::blocking_mutex::Mutex::new(RefCell::new(Vec::new()));

static SUB_QUEUE: Channel<CriticalSectionRawMutex, Subscription, QUEUE_DEPTH> = Channel::new();

/// Adds a subscription and queues it for the active connection.
///
/// If the connection is currently down, the subscription will be registered
/// automatically as soon as it reconnects.
pub async fn subscribe(topic: impl Into<String>, qos: QoS) {
    let sub = Subscription {
        topic: topic.into(),
        qos,
    };
    SUBSCRIPTIONS.lock(|list| {
        list.borrow_mut().push(sub.clone());
    });
    SUB_QUEUE.send(sub).await;
}

/// Queues a message for publishing as soon as the connection is up.
///
/// Safe to call from any task, including ones that have nothing to do with the
/// MQTT connection itself — this is the intended way for other modules to send
/// MQTT messages without reaching into the socket owned by [`task`].
pub async fn publish(
    topic: impl Into<String>,
    payload: impl Into<Vec<u8>>,
    qos: QoS,
    retain: bool,
) {
    OUTGOING
        .send(OutgoingMessage {
            topic: topic.into(),
            payload: payload.into(),
            qos,
            retain,
        })
        .await;
}

/// Waits for the next message received from the broker.
///
/// This is the only way incoming messages are surfaced; `crate::app::mqtt_handler`
/// is the single task expected to call this in a loop.
pub async fn receive() -> IncomingMessage {
    INCOMING.receive().await
}

#[derive(Debug)]
pub struct TcpTransportError(embassy_net::tcp::Error);

impl mqtt_async_embedded::transport::TransportError for TcpTransportError {}

struct TcpTransport<'a> {
    socket: TcpSocket<'a>,
}

impl MqttTransport for TcpTransport<'_> {
    type Error = TcpTransportError;

    async fn send(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.socket.write_all(buf).await.map_err(TcpTransportError)
    }

    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.socket.read(buf).await.map_err(TcpTransportError)
    }
}

async fn resolve(stack: Stack<'_>, host: &str) -> Option<IpAddr> {
    if let Ok(addr) = host.parse::<IpAddr>() {
        return Some(addr);
    }

    match stack.dns_query(host, DnsQueryType::A).await {
        Ok(addrs) => addrs.first().map(|addr| (*addr).into()),
        Err(err) => {
            log::warn!("Failed to resolve MQTT host {host:?}: {err:?}");
            None
        }
    }
}

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>, cfg: AppConfig) {
    loop {
        if let Err(err) = run_once(stack, &cfg).await {
            log::warn!("MQTT connection lost: {err}");
        }
        Timer::after(RECONNECT_DELAY).await;
    }
}

async fn run_once(stack: Stack<'static>, cfg: &AppConfig) -> Result<(), &'static str> {
    let ip = resolve(stack, &cfg.mqtt_host)
        .await
        .ok_or("could not resolve MQTT host")?;

    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 1024];
    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);

    socket
        .connect(IpEndpoint::new(ip.into(), cfg.mqtt_port))
        .await
        .map_err(|_| "TCP connect to MQTT broker failed")?;

    let transport = TcpTransport { socket };

    let client_id = "mqtt_gate";
    let mut options = MqttOptions::new(client_id, &cfg.mqtt_host, cfg.mqtt_port)
        .with_keep_alive(Duration::from_secs(50));
    if let (Some(username), Some(password)) = (&cfg.mqtt_username, &cfg.mqtt_password) {
        options = options.with_credentials(username, password.as_bytes());
    }

    let mut client = MqttClient::<_, MAX_TOPICS, MQTT_BUF_SIZE>::new(transport, options);
    client.connect().await.map_err(|_| "MQTT CONNECT failed")?;
    log::info!("Connected to MQTT broker at {}", cfg.mqtt_host);

    let existing_subs = SUBSCRIPTIONS.lock(|list| {
        while SUB_QUEUE.try_receive().is_ok() {}
        list.borrow().clone()
    });
    for sub in &existing_subs {
        client
            .subscribe(&sub.topic, sub.qos)
            .await
            .map_err(|_| "MQTT re-subscribe failed")?;
        log::info!("Re-subscribed to {}", sub.topic);
    }

    loop {
        match select(client.poll(), select(OUTGOING.receive(), SUB_QUEUE.receive())).await {
            Either::First(poll_result) => {
                if let Some(MqttEvent::Publish(p)) =
                    poll_result.map_err(|_| "MQTT connection error")?
                {
                    let message = IncomingMessage {
                        topic: String::from(p.topic),
                        payload: Vec::from(p.payload),
                    };
                    INCOMING.send(message).await;
                }
            }
            Either::Second(Either::First(out)) => {
                client
                    .publish(&out.topic, &out.payload, out.qos, out.retain)
                    .await
                    .map_err(|_| "MQTT publish failed")?;
            }
            Either::Second(Either::Second(sub)) => {
                client
                    .subscribe(&sub.topic, sub.qos)
                    .await
                    .map_err(|_| "MQTT subscribe failed")?;
                log::info!("Subscribed to {}", sub.topic);
            }
        }
    }
}
