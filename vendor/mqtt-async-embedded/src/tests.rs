//! Wire-level tests against a scripted in-memory transport: every packet the
//! client sends is compared byte for byte with MQTT 3.1.1, and the broker side
//! is replayed in arbitrary TCP chunks (split packets, several packets per read).

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::vec::Vec;

use embassy_futures::block_on;
use embassy_time::{Duration, MockDriver};

use crate::client::{MqttClient, MqttEvent, MqttOptions};
use crate::error::{ConnectReasonCode, MqttError, ProtocolError};
use crate::packet::{DecodePacket, EncodePacket, Publish, QoS, SubAck};
use crate::transport::{MqttTransport, TransportError};
use crate::util;

#[derive(Debug, PartialEq)]
struct Closed;

impl TransportError for Closed {}

/// Hands out the scripted chunks one `recv` at a time; reports the
/// connection as closed once they're used up, so a test can never hang.
#[derive(Default)]
struct Script {
    incoming: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
}

impl Script {
    fn with(chunks: &[&[u8]]) -> Self {
        Script {
            incoming: chunks.iter().map(|chunk| chunk.to_vec()).collect(),
            sent: Vec::new(),
        }
    }
}

impl MqttTransport for &mut Script {
    type Error = Closed;

    async fn send(&mut self, buf: &[u8]) -> Result<(), Closed> {
        self.sent.push(buf.to_vec());
        Ok(())
    }

    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Closed> {
        let mut chunk = self.incoming.pop_front().ok_or(Closed)?;
        if chunk.len() > buf.len() {
            // Deliver what fits, keep the rest for the next read.
            self.incoming.push_front(chunk.split_off(buf.len()));
        }
        buf[..chunk.len()].copy_from_slice(&chunk);
        Ok(chunk.len())
    }
}

/// The mock clock is process-wide; tests that use a client must not run
/// concurrently, or one test's time jumps would trigger another's keep-alive.
static CLOCK: Mutex<()> = Mutex::new(());

fn lock_clock() -> MutexGuard<'static, ()> {
    let guard = CLOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    MockDriver::get().reset();
    guard
}

const CONNACK_OK: &[u8] = &[0x20, 0x02, 0x00, 0x00];
const BUF: usize = 256;

fn options() -> MqttOptions<'static> {
    MqttOptions::new("gate", "broker", 1883).with_keep_alive(Duration::from_secs(50))
}

/// A client that has completed the CONNECT handshake; `chunks` follow the CONNACK.
fn connected(script: &mut Script) -> MqttClient<'static, &mut Script, 4, BUF> {
    script.incoming.push_front(CONNACK_OK.to_vec());
    let mut client = MqttClient::new(script, options());
    block_on(client.connect()).expect("connect");
    client
}

/// Encodes a PUBLISH the way a broker would send it.
fn publish_bytes(topic: &str, payload: &[u8], qos: QoS, packet_id: Option<u16>) -> Vec<u8> {
    let mut buf = [0u8; 1024];
    let len = Publish {
        topic,
        qos,
        retain: false,
        payload,
        packet_id,
    }
    .encode(&mut buf)
    .unwrap();
    buf[..len].to_vec()
}

fn expect_publish(event: Option<MqttEvent<'_>>) -> (String, Vec<u8>) {
    match event {
        Some(MqttEvent::Publish(p)) => (p.topic.to_string(), p.payload.to_vec()),
        None => panic!("expected a PUBLISH event"),
    }
}

// --- CONNECT / CONNACK ---

#[test]
fn connect_without_credentials_is_mqtt_311() {
    let _clock = lock_clock();
    let mut script = Script::with(&[CONNACK_OK]);
    block_on(MqttClient::<_, 4, BUF>::new(&mut script, options()).connect()).unwrap();

    #[rustfmt::skip]
    let expected = [
        0x10, 16,                          // CONNECT, remaining length
        0x00, 0x04, b'M', b'Q', b'T', b'T', // protocol name
        0x04,                              // level 4 = 3.1.1
        0x02,                              // clean session
        0x00, 50,                          // keep-alive seconds
        0x00, 0x04, b'g', b'a', b't', b'e', // client id
    ];
    assert_eq!(script.sent, [expected.to_vec()]);
}

#[test]
fn connect_sends_username_and_password() {
    let _clock = lock_clock();
    let mut script = Script::with(&[CONNACK_OK]);
    let options = options().with_credentials("ha", b"pw");
    block_on(MqttClient::<_, 4, BUF>::new(&mut script, options).connect()).unwrap();

    let sent = &script.sent[0];
    assert_eq!(sent[1] as usize, sent.len() - 2);
    assert_eq!(sent[9], 0xC2, "username + password + clean session flags");
    assert!(sent.ends_with(&[0x00, 0x02, b'h', b'a', 0x00, 0x02, b'p', b'w']));
}

#[test]
fn connect_reports_refusal_reason() {
    let _clock = lock_clock();
    let mut script = Script::with(&[&[0x20, 0x02, 0x00, 0x04]]);
    let result = block_on(MqttClient::<_, 4, BUF>::new(&mut script, options()).connect());
    assert!(matches!(
        result,
        Err(MqttError::ConnectionRefused(
            ConnectReasonCode::BadUserNameOrPassword
        ))
    ));
}

#[test]
fn connack_split_across_reads() {
    let _clock = lock_clock();
    let mut script = Script::with(&[&[0x20], &[0x02, 0x00], &[0x00]]);
    block_on(MqttClient::<_, 4, BUF>::new(&mut script, options()).connect()).unwrap();
}

#[test]
fn connect_fails_cleanly_on_short_connack() {
    let _clock = lock_clock();
    let mut script = Script::with(&[&[0x20, 0x00]]);
    let result = block_on(MqttClient::<_, 4, BUF>::new(&mut script, options()).connect());
    assert!(matches!(
        result,
        Err(MqttError::Protocol(ProtocolError::MalformedPacket))
    ));
}

#[test]
fn connect_fails_when_broker_closes() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let result = block_on(MqttClient::<_, 4, BUF>::new(&mut script, options()).connect());
    assert!(matches!(result, Err(MqttError::Transport(Closed))));
}

// --- Outgoing PUBLISH / SUBSCRIBE ---

#[test]
fn publish_qos0_retained() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let mut client = connected(&mut script);
    block_on(client.publish("a/b", b"ON", QoS::AtMostOnce, true)).unwrap();
    drop(client);

    assert_eq!(
        script.sent[1],
        [0x31, 7, 0x00, 0x03, b'a', b'/', b'b', b'O', b'N']
    );
}

#[test]
fn publish_qos1_carries_increasing_nonzero_packet_ids() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let mut client = connected(&mut script);
    block_on(client.publish("t", b"x", QoS::AtLeastOnce, false)).unwrap();
    block_on(client.publish("t", b"x", QoS::AtLeastOnce, false)).unwrap();
    drop(client);

    let first = &script.sent[1];
    let second = &script.sent[2];
    assert_eq!(first[0], 0x32);
    let id = |packet: &[u8]| u16::from_be_bytes([packet[5], packet[6]]);
    assert_ne!(id(first), 0);
    assert_eq!(id(second), id(first) + 1);
}

#[test]
fn long_publish_uses_two_byte_remaining_length_and_round_trips() {
    let payload = [b'j'; 300];
    let bytes = publish_bytes(
        "homeassistant/cover/x/config",
        &payload,
        QoS::AtLeastOnce,
        Some(7),
    );
    assert_eq!(
        util::complete_packet_len(&bytes).unwrap(),
        Some(bytes.len())
    );
    assert!(
        bytes[1] & 0x80 != 0,
        "remaining length > 127 needs two bytes"
    );

    let decoded = Publish::decode(&bytes).unwrap();
    assert_eq!(decoded.topic, "homeassistant/cover/x/config");
    assert_eq!(decoded.payload, payload);
    assert_eq!(decoded.packet_id, Some(7));
}

#[test]
fn publish_larger_than_buffer_is_an_error_not_a_panic() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let mut client = connected(&mut script);
    let payload = [0u8; BUF];
    let result = block_on(client.publish("t", &payload, QoS::AtMostOnce, false));
    assert!(matches!(result, Err(MqttError::BufferTooSmall)));
}

#[test]
fn subscribe_packet() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let mut client = connected(&mut script);
    block_on(client.subscribe("x/set", QoS::AtLeastOnce)).unwrap();
    drop(client);

    let sent = &script.sent[1];
    assert_eq!(sent[0], 0x82);
    assert_eq!(sent[1] as usize, sent.len() - 2);
    assert_eq!(&sent[4..], [0x00, 0x05, b'x', b'/', b's', b'e', b't', 0x01]);
}

// --- Incoming packets ---

#[test]
fn poll_delivers_publish() {
    let _clock = lock_clock();
    let message = publish_bytes("gate/set", b"OPEN", QoS::AtMostOnce, None);
    let mut script = Script::with(&[&message]);
    let mut client = connected(&mut script);

    let (topic, payload) = expect_publish(block_on(client.poll()).unwrap());
    assert_eq!(topic, "gate/set");
    assert_eq!(payload, b"OPEN");
}

#[test]
fn poll_separates_packets_arriving_in_one_read() {
    let _clock = lock_clock();
    let suback = [0x90, 0x03, 0x00, 0x02, 0x01];
    let mut chunk = suback.to_vec();
    chunk.extend(publish_bytes("a", b"OPEN", QoS::AtMostOnce, None));
    chunk.extend(publish_bytes("b", b"STOP", QoS::AtMostOnce, None));
    let mut script = Script::with(&[&chunk]);
    let mut client = connected(&mut script);

    assert!(block_on(client.poll()).unwrap().is_none(), "SUBACK");
    assert_eq!(
        expect_publish(block_on(client.poll()).unwrap()),
        ("a".into(), b"OPEN".to_vec())
    );
    assert_eq!(
        expect_publish(block_on(client.poll()).unwrap()),
        ("b".into(), b"STOP".to_vec())
    );
}

#[test]
fn poll_reassembles_publish_split_across_reads() {
    let _clock = lock_clock();
    let message = publish_bytes("gate/set", b"CLOSE", QoS::AtMostOnce, None);
    let (head, tail) = message.split_at(5);
    let mut script = Script::with(&[head, tail]);
    let mut client = connected(&mut script);

    assert_eq!(
        expect_publish(block_on(client.poll()).unwrap()),
        ("gate/set".into(), b"CLOSE".to_vec())
    );
}

#[test]
fn poll_acknowledges_qos1_publish() {
    let _clock = lock_clock();
    let message = publish_bytes("gate/set", b"OPEN", QoS::AtLeastOnce, Some(0x1234));
    let mut script = Script::with(&[&message]);
    let mut client = connected(&mut script);

    expect_publish(block_on(client.poll()).unwrap());
    drop(client);
    assert_eq!(script.sent.last().unwrap(), &[0x40, 0x02, 0x12, 0x34]);
}

#[test]
fn poll_ignores_unknown_packet_types() {
    let _clock = lock_clock();
    // UNSUBACK - never expected, but must not tear down the connection.
    let mut chunk = vec![0xB0, 0x02, 0x00, 0x01];
    chunk.extend(publish_bytes("a", b"ON", QoS::AtMostOnce, None));
    let mut script = Script::with(&[&chunk]);
    let mut client = connected(&mut script);

    assert!(block_on(client.poll()).unwrap().is_none());
    expect_publish(block_on(client.poll()).unwrap());
}

#[test]
fn poll_rejects_malformed_remaining_length() {
    let _clock = lock_clock();
    let mut script = Script::with(&[&[0x30, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]]);
    let mut client = connected(&mut script);
    assert!(matches!(
        block_on(client.poll()),
        Err(MqttError::Protocol(ProtocolError::MalformedPacket))
    ));
}

#[test]
fn poll_sends_pingreq_after_keep_alive_idle() {
    let _clock = lock_clock();
    let mut script = Script::with(&[&[0xD0, 0x00], &[0xD0, 0x00]]);
    let mut client = connected(&mut script);

    MockDriver::get().advance(Duration::from_secs(49));
    assert!(block_on(client.poll()).unwrap().is_none());
    MockDriver::get().advance(Duration::from_secs(1));
    assert!(block_on(client.poll()).unwrap().is_none());
    drop(client);

    // CONNECT, then exactly one PINGREQ (only once 50s had passed).
    assert_eq!(script.sent.len(), 2);
    assert_eq!(script.sent[1], [0xC0, 0x00]);
}

#[test]
fn poll_after_broker_closed_is_an_error() {
    let _clock = lock_clock();
    let mut script = Script::default();
    let mut client = connected(&mut script);
    assert!(matches!(
        block_on(client.poll()),
        Err(MqttError::Transport(Closed))
    ));
}

// --- Codec helpers ---

#[test]
fn suback_decodes_packet_id_and_return_codes() {
    let suback = SubAck::decode(&[0x90, 0x04, 0x12, 0x34, 0x01, 0x80]).unwrap();
    assert_eq!(suback.packet_id, 0x1234);
    assert_eq!(suback.reason_codes.as_slice(), [0x01, 0x80]);
}

#[test]
fn variable_byte_integer_round_trips_at_every_boundary() {
    for value in [
        0,
        127,
        128,
        16_383,
        16_384,
        2_097_151,
        2_097_152,
        268_435_455,
    ] {
        let mut buf = [0u8; 4];
        let written = util::write_variable_byte_integer_len(&mut buf, value).unwrap();
        let mut cursor = 0;
        assert_eq!(
            util::read_variable_byte_integer(&mut cursor, &buf).unwrap(),
            value
        );
        assert_eq!(cursor, written);
    }
}

#[test]
fn complete_packet_len_waits_for_the_whole_packet() {
    assert_eq!(util::complete_packet_len(&[]).unwrap(), None);
    assert_eq!(util::complete_packet_len(&[0x30]).unwrap(), None);
    assert_eq!(
        util::complete_packet_len(&[0x30, 0x02, 0x00]).unwrap(),
        None
    );
    assert_eq!(
        util::complete_packet_len(&[0x30, 0x02, 0x00, 0x00, 0xD0]).unwrap(),
        Some(4)
    );
    assert_eq!(util::complete_packet_len(&[0xD0, 0x00]).unwrap(), Some(2));
}
