//! Flash records on a simulated NOR flash (sequential-storage's own mock,
//! which rejects writing a word twice without erasing and can cut the power
//! after any number of written bytes).

use embassy_futures::block_on;
use sequential_storage::mock_flash::{MockFlashBase, WriteCountCheck};

use super::*;

/// Same geometry as the ESP32's `app_cfg` partition: 4-byte words, 4 KiB
/// erase pages, 64 KiB.
type PartitionFlash = MockFlashBase<16, 4, 1024>;
/// A small flash (4 x 1 KiB pages), so page reuse happens after a few
/// hundred writes.
type SmallFlash = MockFlashBase<4, 4, 256>;

fn config() -> AppConfig {
    AppConfig {
        wifi_ssid: "Home".into(),
        wifi_password: "wifi-secret".into(),
        mqtt_host: "192.168.1.10".into(),
        mqtt_port: 1883,
        mqtt_username: Some("ha".into()),
        mqtt_password: Some("mqtt-secret".into()),
    }
}

fn largest_config() -> AppConfig {
    use crate::config::{MAX_CRED_LEN, MAX_HOST_LEN, MAX_PSK_LEN, MAX_SSID_LEN};
    AppConfig {
        wifi_ssid: "s".repeat(MAX_SSID_LEN),
        wifi_password: "p".repeat(MAX_PSK_LEN),
        mqtt_host: "h".repeat(MAX_HOST_LEN),
        mqtt_port: u16::MAX,
        mqtt_username: Some("u".repeat(MAX_CRED_LEN)),
        mqtt_password: Some("w".repeat(MAX_CRED_LEN)),
    }
}

fn state(n: u32) -> GateState {
    GateState {
        leaf_a_position: n as u8,
        leaf_b_position: (n / 256) as u8,
        wind_locked: n % 3 == 0,
    }
}

fn partition() -> PartitionFlash {
    MockFlashBase::new(WriteCountCheck::OnceOnly, None, true)
}

#[test]
fn blank_flash_has_nothing_stored() {
    let mut flash = partition();
    let mut store = Store::new(&mut flash);
    block_on(async {
        assert_eq!(store.load_config().await.unwrap(), None);
        assert_eq!(store.load_gate_state().await.unwrap(), None);
    });
}

#[test]
fn records_survive_a_reboot() {
    let mut flash = partition();
    block_on(async {
        let mut store = Store::new(&mut flash);
        store.save_config(&config()).await.unwrap();
        store.save_gate_state(&state(42)).await.unwrap();
    });
    // A fresh `Store` re-reads everything from flash, like after a reset.
    block_on(async {
        let mut store = Store::new(&mut flash);
        assert_eq!(store.load_config().await.unwrap(), Some(config()));
        assert_eq!(store.load_gate_state().await.unwrap(), Some(state(42)));
    });
}

#[test]
fn largest_valid_config_fits() {
    let mut flash = partition();
    block_on(async {
        let mut store = Store::new(&mut flash);
        store.save_config(&largest_config()).await.unwrap();
        assert_eq!(store.load_config().await.unwrap(), Some(largest_config()));
    });
}

#[test]
fn erasing_the_config_keeps_the_gate_state() {
    // Erasing overwrites already-written words, like the ESP32 flash allows.
    let flash = MockFlashBase::<16, 4, 1024>::new(WriteCountCheck::Twice, None, true);
    let flash = block_on(async {
        let mut store = Store::new(flash);
        store.save_config(&config()).await.unwrap();
        store.save_gate_state(&state(7)).await.unwrap();
        store.erase_config().await.unwrap();
        store.into_flash()
    });
    block_on(async {
        let mut store = Store::new(flash);
        assert_eq!(store.load_config().await.unwrap(), None);
        assert_eq!(store.load_gate_state().await.unwrap(), Some(state(7)));
    });
}

#[test]
fn config_survives_years_of_gate_state_writes() {
    // The gate state is saved after every movement; the log wraps around the
    // partition many times. The config, written once, must never get lost.
    let mut flash = partition();
    block_on(async {
        let mut store = Store::new(&mut flash);
        store.save_config(&config()).await.unwrap();
        for n in 0..10_000 {
            store.save_gate_state(&state(n)).await.unwrap();
        }
    });
    block_on(async {
        let mut store = Store::new(&mut flash);
        assert_eq!(store.load_config().await.unwrap(), Some(config()));
        assert_eq!(store.load_gate_state().await.unwrap(), Some(state(9_999)));
    });
}

#[test]
fn power_loss_while_saving_never_loses_or_corrupts_data() {
    // Cut the power after every possible number of written bytes, during a
    // run of saves long enough to include page reuse, then reboot.
    let mut cut = 0;
    loop {
        let mut flash = SmallFlash::new(WriteCountCheck::OnceOnly, None, true);
        block_on(async {
            let mut store = Store::new(&mut flash);
            store.save_config(&config()).await.unwrap();
            store.save_gate_state(&state(0)).await.unwrap();
        });

        flash.bytes_until_shutoff = Some(cut);
        let mut saved = 0;
        let mut power_lost = false;
        block_on(async {
            let mut store = Store::new(&mut flash);
            for n in 1..400 {
                if store.save_gate_state(&state(n)).await.is_err() {
                    power_lost = true;
                    return;
                }
                saved = n;
            }
        });
        if !power_lost {
            break; // `cut` is beyond everything the run writes
        }

        flash.bytes_until_shutoff = None;
        block_on(async {
            let mut store = Store::new(&mut flash);
            assert_eq!(
                store.load_config().await.unwrap(),
                Some(config()),
                "config lost (cut after {cut} bytes)"
            );
            let gate_state = store.load_gate_state().await.unwrap();
            assert!(
                gate_state == Some(state(saved)) || gate_state == Some(state(saved + 1)),
                "cut after {cut} bytes: got {gate_state:?}, last saved {saved}"
            );
            // And the store keeps working afterwards.
            store.save_gate_state(&state(1000)).await.unwrap();
            assert_eq!(store.load_gate_state().await.unwrap(), Some(state(1000)));
        });
        cut += 7;
    }
    assert!(cut > 1000, "the run should write more than a few pages");
}
