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
//!   socket, which stays owned by this task alone. It never blocks: while the
//!   broker is unreachable the oldest queued messages are dropped, so a dead
//!   connection can never stall the caller (e.g. the gate control loop).
//! - [`wait_connected`] fires after every (re)connect, so the app can
//!   republish discovery configs and current state that may have been dropped
//!   while offline.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::net::IpAddr;

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, TrySendError};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use mqtt_async_embedded::client::{MqttClient, MqttEvent, MqttOptions};
use mqtt_async_embedded::packet::QoS;
use mqtt_async_embedded::transport::MqttTransport;

use app_core::config::AppConfig;
use app_core::mqtt::MQTT_BUFFER_SIZE;

const MAX_TOPICS: usize = 4;
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// How many incoming messages/subscriptions may be queued before the producer
/// has to wait.
const QUEUE_DEPTH: usize = 8;
/// How many outgoing messages are buffered; beyond that the oldest is dropped.
/// Big enough for a full republish after reconnect (discovery + state).
const OUTGOING_DEPTH: usize = 16;
const KEEP_ALIVE: Duration = Duration::from_secs(50);
/// How often the connection loop wakes up to give `MqttClient::poll` a chance
/// to send a due PINGREQ. Keep-alive + this must stay well below 1.5x keep-alive.
const KEEP_ALIVE_CHECK: Duration = Duration::from_secs(10);
/// A broker that stops answering for this long is treated as gone.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(90);

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
static OUTGOING: Channel<CriticalSectionRawMutex, OutgoingMessage, OUTGOING_DEPTH> = Channel::new();
static CONNECTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[derive(Clone, Debug)]
pub struct Subscription {
    pub topic: String,
    pub qos: QoS,
}

static SUBSCRIPTIONS: embassy_sync::blocking_mutex::Mutex<
    CriticalSectionRawMutex,
    RefCell<Vec<Subscription>>,
> = embassy_sync::blocking_mutex::Mutex::new(RefCell::new(Vec::new()));

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
/// MQTT messages without reaching into the socket owned by [`task`]. Never
/// blocks: if the queue is full (broker unreachable), the oldest queued
/// message is dropped to make room.
pub fn publish(topic: impl Into<String>, payload: impl Into<Vec<u8>>, qos: QoS, retain: bool) {
    let mut message = OutgoingMessage {
        topic: topic.into(),
        payload: payload.into(),
        qos,
        retain,
    };
    loop {
        match OUTGOING.try_send(message) {
            Ok(()) => return,
            Err(TrySendError::Full(rejected)) => {
                message = rejected;
                if let Ok(dropped) = OUTGOING.try_receive() {
                    log::warn!("MQTT queue full - dropping message for {}", dropped.topic);
                }
            }
        }
    }
}

/// Waits until the next successful (re)connect to the broker, including
/// re-subscription. Intended for a single task that republishes state.
pub async fn wait_connected() {
    CONNECTED.wait().await
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
        match self.socket.read(buf).await {
            // EOF: the broker closed the connection. Reported as an error, because
            // `MqttClient::poll` treats `Ok(0)` as "nothing yet" and would spin on
            // it forever without ever yielding to the executor.
            Ok(0) => Err(TcpTransportError(embassy_net::tcp::Error::ConnectionReset)),
            result => result.map_err(TcpTransportError),
        }
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
    // Wi-Fi may be down or still reconnecting - just wait for it.
    stack.wait_config_up().await;

    let ip = resolve(stack, &cfg.mqtt_host)
        .await
        .ok_or("could not resolve MQTT host")?;

    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 1024];
    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(SOCKET_TIMEOUT));

    socket
        .connect(IpEndpoint::new(ip.into(), cfg.mqtt_port))
        .await
        .map_err(|_| "TCP connect to MQTT broker failed")?;

    let transport = TcpTransport { socket };

    let client_id = "mqtt_gate";
    let mut options =
        MqttOptions::new(client_id, &cfg.mqtt_host, cfg.mqtt_port).with_keep_alive(KEEP_ALIVE);
    if let (Some(username), Some(password)) = (&cfg.mqtt_username, &cfg.mqtt_password) {
        options = options.with_credentials(username, password.as_bytes());
    }

    let mut client = MqttClient::<_, MAX_TOPICS, MQTT_BUFFER_SIZE>::new(transport, options);
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

    // Whatever piled up while offline is stale; the app republishes the
    // current state in response to CONNECTED.
    while OUTGOING.try_receive().is_ok() {}
    CONNECTED.signal(());

    loop {
        match select3(
            client.poll(),
            select(OUTGOING.receive(), SUB_QUEUE.receive()),
            // `poll` only sends its keep-alive PINGREQ on entry, but then sits in
            // `recv` indefinitely on an idle connection - so re-enter it
            // periodically, or the broker drops us after 1.5x the keep-alive.
            Timer::after(KEEP_ALIVE_CHECK),
        )
        .await
        {
            Either3::Third(()) => {}
            Either3::First(poll_result) => {
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
            Either3::Second(Either::First(out)) => {
                client
                    .publish(&out.topic, &out.payload, out.qos, out.retain)
                    .await
                    .map_err(|_| "MQTT publish failed")?;
            }
            Either3::Second(Either::Second(sub)) => {
                client
                    .subscribe(&sub.topic, sub.qos)
                    .await
                    .map_err(|_| "MQTT subscribe failed")?;
                log::info!("Subscribed to {}", sub.topic);
            }
        }
    }
}
