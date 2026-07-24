//! Persists `AppConfig` to a dedicated flash data partition (`app_cfg`, see
//! `partitions.csv`) using `sequential-storage`'s log-structured key-value map, so
//! writes survive power loss without needing wear-leveling logic of our own.
//!
//! `esp_storage::FlashStorage::new` panics if constructed more than once, so callers
//! must create exactly one `FlashStorage` at boot (see `src/bin/main.rs`) and pass a
//! `&mut` reference into `load`/`save`/`erase` rather than each function constructing
//! its own.

use embassy_embedded_hal::adapter::BlockingAsync;
use esp_bootloader_esp_idf::partitions::{
    self, DataPartitionSubType, PartitionEntry, PartitionType,
};
use esp_storage::FlashStorage;
use sequential_storage::cache::NoCache;
use sequential_storage::map::{MapConfig, MapStorage};
use serde::{Deserialize, Serialize};

use crate::infra::config::AppConfig;

const CONFIG_KEY: u8 = 1;
const GATE_STATE_KEY: u8 = 2;

/// Gate position and wind-lock latch, persisted so a power loss doesn't lose
/// track of where the leaves were or whether the wind lock was engaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GateState {
    pub left_position: u8,
    pub right_position: u8,
    pub wind_locked: bool,
}

impl sequential_storage::map::PostcardValue<'_> for GateState {}
/// Large enough to cover our whole (4-entry) partition table; doesn't need to reach
/// `partitions::PARTITION_TABLE_MAX_LEN` since our entries are all near the start.
const PARTITION_TABLE_SCRATCH_LEN: usize = 512;
/// Must fit the largest serialized `AppConfig` plus its key and item header.
const DATA_BUFFER_LEN: usize = 512;

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

fn find_app_cfg_partition<'a>(
    flash: &mut FlashStorage<'_>,
    scratch: &'a mut [u8; PARTITION_TABLE_SCRATCH_LEN],
) -> Result<PartitionEntry<'a>, StorageError> {
    let table =
        partitions::read_partition_table(flash, scratch).map_err(StorageError::Partition)?;
    table
        .find_partition(PartitionType::Data(DataPartitionSubType::Undefined))
        .map_err(StorageError::Partition)?
        .ok_or(StorageError::PartitionNotFound)
}

/// Loads the stored config, if any. Missing partition data, a missing item, or a
/// deserialization error are all treated the same way: "nothing is configured yet",
/// which is what tells `main()` to fall into provisioning mode.
pub async fn load(flash: &mut FlashStorage<'_>) -> Option<AppConfig> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let entry = match find_app_cfg_partition(flash, &mut scratch) {
        Ok(entry) => entry,
        Err(err) => {
            log::warn!("app_cfg partition lookup failed: {err:?}");
            return None;
        }
    };
    let len = entry.len();
    let region = entry.as_embedded_storage(flash);
    let async_flash = BlockingAsync::new(region);
    let mut storage =
        MapStorage::<u8, _, _>::new(async_flash, MapConfig::new(0..len), NoCache::new());
    let mut data_buffer = [0u8; DATA_BUFFER_LEN];

    match storage
        .fetch_item::<AppConfig>(&mut data_buffer, &CONFIG_KEY)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            log::warn!("Reading stored config failed: {err:?}");
            None
        }
    }
}

pub async fn save(flash: &mut FlashStorage<'_>, cfg: &AppConfig) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let entry = find_app_cfg_partition(flash, &mut scratch)?;
    let len = entry.len();
    let region = entry.as_embedded_storage(flash);
    let async_flash = BlockingAsync::new(region);
    let mut storage =
        MapStorage::<u8, _, _>::new(async_flash, MapConfig::new(0..len), NoCache::new());
    let mut data_buffer = [0u8; DATA_BUFFER_LEN];

    storage
        .store_item(&mut data_buffer, &CONFIG_KEY, cfg)
        .await?;
    Ok(())
}

/// Loads the persisted gate position/wind-lock state, if any. Missing data is
/// treated as "never saved", which is what tells the caller to fall back to
/// `GateState::default()` (closed, unlocked).
pub async fn load_gate_state(flash: &mut FlashStorage<'_>) -> Option<GateState> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let entry = match find_app_cfg_partition(flash, &mut scratch) {
        Ok(entry) => entry,
        Err(err) => {
            log::warn!("app_cfg partition lookup failed: {err:?}");
            return None;
        }
    };
    let len = entry.len();
    let region = entry.as_embedded_storage(flash);
    let async_flash = BlockingAsync::new(region);
    let mut storage =
        MapStorage::<u8, _, _>::new(async_flash, MapConfig::new(0..len), NoCache::new());
    let mut data_buffer = [0u8; DATA_BUFFER_LEN];

    match storage
        .fetch_item::<GateState>(&mut data_buffer, &GATE_STATE_KEY)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            log::warn!("Reading stored gate state failed: {err:?}");
            None
        }
    }
}

pub async fn save_gate_state(
    flash: &mut FlashStorage<'_>,
    state: &GateState,
) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let entry = find_app_cfg_partition(flash, &mut scratch)?;
    let len = entry.len();
    let region = entry.as_embedded_storage(flash);
    let async_flash = BlockingAsync::new(region);
    let mut storage =
        MapStorage::<u8, _, _>::new(async_flash, MapConfig::new(0..len), NoCache::new());
    let mut data_buffer = [0u8; DATA_BUFFER_LEN];

    storage
        .store_item(&mut data_buffer, &GATE_STATE_KEY, state)
        .await?;
    Ok(())
}

/// Clears the stored config so the next boot falls into provisioning mode.
pub async fn erase(flash: &mut FlashStorage<'_>) -> Result<(), StorageError> {
    let mut scratch = [0u8; PARTITION_TABLE_SCRATCH_LEN];
    let entry = find_app_cfg_partition(flash, &mut scratch)?;
    let len = entry.len();
    let region = entry.as_embedded_storage(flash);
    let async_flash = BlockingAsync::new(region);
    let mut storage =
        MapStorage::<u8, _, _>::new(async_flash, MapConfig::new(0..len), NoCache::new());
    let mut data_buffer = [0u8; DATA_BUFFER_LEN];

    storage.remove_item(&mut data_buffer, &CONFIG_KEY).await?;
    Ok(())
}
