//! Persists `AppConfig` and `GateState` in the dedicated flash data partition
//! (`app_cfg`, see `partitions.csv`). The record format (a `sequential-storage`
//! log-structured map with wear-leveling and power-loss safety) lives in
//! `app_core::storage`, where it's tested; this module only finds the
//! partition.
//!
//! `esp_storage::FlashStorage::new` panics if constructed more than once, so callers
//! must create exactly one `FlashStorage` at boot (see `src/bin/main.rs`, where it's
//! wrapped in a [`SharedFlash`] mutex) and pass a `&mut` reference into
//! these functions rather than each function constructing its own.

use app_core::config::AppConfig;
use app_core::storage::{GateState, Store};
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_bootloader_esp_idf::partitions::{self, DataPartitionSubType, FlashRegion, PartitionType};
use esp_storage::FlashStorage;

/// The one `FlashStorage` instance, shared between whoever needs flash at
/// runtime (the gate task persisting its state, provisioning saving config).
pub type SharedFlash = Mutex<CriticalSectionRawMutex, FlashStorage<'static>>;

/// Large enough to cover our whole (4-entry) partition table; doesn't need to reach
/// `partitions::PARTITION_TABLE_MAX_LEN` since our entries are all near the start.
const PARTITION_TABLE_SCRATCH_LEN: usize = 512;

#[derive(Debug)]
pub enum StorageError {
    Partition(partitions::Error),
    PartitionNotFound,
    Storage(sequential_storage::Error<partitions::Error>),
}

impl From<sequential_storage::Error<partitions::Error>> for StorageError {
    fn from(e: sequential_storage::Error<partitions::Error>) -> Self {
        StorageError::Storage(e)
    }
}

type AppCfgStore<'a, 'f> = Store<BlockingAsync<FlashRegion<'a, FlashStorage<'f>>>>;

fn open<'a, 'f>(
    flash: &'a mut FlashStorage<'f>,
    scratch: &'a mut [u8; PARTITION_TABLE_SCRATCH_LEN],
) -> Result<AppCfgStore<'a, 'f>, StorageError> {
    let table =
        partitions::read_partition_table(flash, scratch).map_err(StorageError::Partition)?;
    let entry = table
        .find_partition(PartitionType::Data(DataPartitionSubType::Undefined))
        .map_err(StorageError::Partition)?
        .ok_or(StorageError::PartitionNotFound)?;
    Ok(Store::new(BlockingAsync::new(
        entry.as_embedded_storage(flash),
    )))
}

/// Loads the stored config, if any. Missing partition data, a missing item, or a
/// deserialization error are all treated the same way: "nothing is configured yet",
/// which is what tells `main()` to fall into provisioning mode.
pub async fn load(flash: &mut FlashStorage<'_>) -> Option<AppConfig> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let mut store = open(flash, &mut scratch)
        .inspect_err(|err| log::warn!("app_cfg partition lookup failed: {err:?}"))
        .ok()?;
    store
        .load_config()
        .await
        .inspect_err(|err| log::warn!("Reading stored config failed: {err:?}"))
        .ok()
        .flatten()
}

pub async fn save(flash: &mut FlashStorage<'_>, cfg: &AppConfig) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    open(flash, &mut scratch)?.save_config(cfg).await?;
    Ok(())
}

/// Loads the persisted gate position/wind-lock state, if any. Missing data is
/// treated as "never saved", which is what tells the caller to fall back to
/// `GateState::default()` (closed, unlocked).
pub async fn load_gate_state(flash: &mut FlashStorage<'_>) -> Option<GateState> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let mut store = open(flash, &mut scratch)
        .inspect_err(|err| log::warn!("app_cfg partition lookup failed: {err:?}"))
        .ok()?;
    store
        .load_gate_state()
        .await
        .inspect_err(|err| log::warn!("Reading stored gate state failed: {err:?}"))
        .ok()
        .flatten()
}

pub async fn save_gate_state(
    flash: &mut FlashStorage<'_>,
    state: &GateState,
) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    open(flash, &mut scratch)?.save_gate_state(state).await?;
    Ok(())
}

/// Clears the stored config so the next boot falls into provisioning mode.
pub async fn erase(flash: &mut FlashStorage<'_>) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    open(flash, &mut scratch)?.erase_config().await?;
    Ok(())
}
