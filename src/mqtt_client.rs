//! Connects to the MQTT broker from the persisted `AppConfig` (host/port and,
//! optionally, username/password) and keeps the connection alive.
//!
//! This only establishes and maintains the CONNECT/keepalive handshake — actual
//! publish/subscribe application logic is out of scope for the provisioning feature
//! and is left for whatever uses this connection next.

use core::net::IpAddr;

use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use mqtt_async_embedded::client::{MqttClient, MqttOptions};
use mqtt_async_embedded::transport::MqttTransport;

use crate::config::AppConfig;

const MAX_TOPICS: usize = 4;
const MQTT_BUF_SIZE: usize = 512;
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

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

    loop {
        client.poll().await.map_err(|_| "MQTT connection error")?;
    }
}
