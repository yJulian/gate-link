//! Minimal DHCP server for clients joining the provisioning hotspot, so a phone or
//! laptop connecting to the AP gets an IP automatically (no captive-portal DNS
//! hijacking - the user is expected to browse to the AP's address directly).

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use edge_dhcp::io::DEFAULT_SERVER_PORT;
use edge_dhcp::server::{Server, ServerOptions};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_net::Stack;
use embassy_time::Instant;

use crate::infra::wifi_ap::AP_IP_OCTETS;

/// Max simultaneously leased clients; the provisioning hotspot only ever expects one.
const MAX_LEASES: usize = 4;
// DHCP packets are small (well under a typical Ethernet MTU); no need for a full 1500B buffer.
const RECV_BUF_LEN: usize = 600;

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>) {
    let server_ip = Ipv4Addr::from(AP_IP_OCTETS);

    // `UdpBuffers`/`Pool` isn't `Sync`, so it can't be a plain `static`; `mk_static!`
    // (backed by `static_cell::StaticCell`, which is `Sync` unconditionally) works around that.
    let udp_buffers = crate::mk_static!(UdpBuffers<1>, UdpBuffers::new());
    let udp = Udp::new(stack, udp_buffers);
    let mut socket = match udp
        .bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
    {
        Ok(socket) => socket,
        Err(err) => {
            log::error!("Failed to bind DHCP server socket: {err:?}");
            return;
        }
    };

    let mut server = Server::<_, MAX_LEASES>::new(|| Instant::now().as_secs(), server_ip);
    let mut gateways = [server_ip];
    let server_options = ServerOptions::new(server_ip, Some(&mut gateways));
    let mut buf = [0u8; RECV_BUF_LEN];

    if let Err(err) =
        edge_dhcp::io::server::run(&mut server, &server_options, &mut socket, &mut buf).await
    {
        log::error!("DHCP server task stopped: {err:?}");
    }
}
