//! The two flash records - [`AppConfig`] and [`GateState`] - kept in one
//! `sequential-storage` log-structured map, so writes get wear-leveling and
//! power-loss safety without logic of our own.
//!
//! Generic over the flash region, so the same code runs against the ESP
//! `app_cfg` partition (see the firmware's `infra::storage`, which only adds
//! the partition lookup) and against a simulated flash in the tests.

use embedded_storage_async::nor_flash::{MultiwriteNorFlash, NorFlash};
use sequential_storage::Error;
use sequential_storage::cache::NoCache;
use sequential_storage::map::{MapConfig, MapStorage};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

const CONFIG_KEY: u8 = 1;
const GATE_STATE_KEY: u8 = 2;

/// Must fit the largest serialized record plus its key and item header.
const DATA_BUFFER_LEN: usize = 512;

/// Gate position and wind-lock latch, persisted so a power loss doesn't lose
/// track of where the leaves were or whether the wind lock was engaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GateState {
    pub leaf_a_position: u8,
    pub leaf_b_position: u8,
    pub wind_locked: bool,
}

impl sequential_storage::map::PostcardValue<'_> for GateState {}

/// The record map over one whole flash region.
pub struct Store<S: NorFlash> {
    map: MapStorage<u8, S, NoCache>,
    buffer: [u8; DATA_BUFFER_LEN],
}

impl<S: NorFlash> Store<S> {
    /// Uses all of `flash`. Panics if its size isn't a multiple of the erase
    /// size, or less than two erase pages.
    pub fn new(flash: S) -> Self {
        let len = flash.capacity() as u32;
        Store {
            map: MapStorage::new(flash, MapConfig::new(0..len), NoCache::new()),
            buffer: [0; DATA_BUFFER_LEN],
        }
    }

    /// `None` if no config was ever saved (or it was erased).
    pub async fn load_config(&mut self) -> Result<Option<AppConfig>, Error<S::Error>> {
        self.map.fetch_item(&mut self.buffer, &CONFIG_KEY).await
    }

    pub async fn save_config(&mut self, config: &AppConfig) -> Result<(), Error<S::Error>> {
        self.map
            .store_item(&mut self.buffer, &CONFIG_KEY, config)
            .await
    }

    /// `None` if the gate state was never saved.
    pub async fn load_gate_state(&mut self) -> Result<Option<GateState>, Error<S::Error>> {
        self.map.fetch_item(&mut self.buffer, &GATE_STATE_KEY).await
    }

    pub async fn save_gate_state(&mut self, state: &GateState) -> Result<(), Error<S::Error>> {
        self.map
            .store_item(&mut self.buffer, &GATE_STATE_KEY, state)
            .await
    }

    /// Hands the flash back, e.g. to simulate a reboot with a fresh `Store`.
    #[cfg(test)]
    fn into_flash(self) -> S {
        self.map.destroy().0
    }
}

impl<S: MultiwriteNorFlash> Store<S> {
    /// Removes the config (so the next boot provisions again), keeping the
    /// gate state.
    pub async fn erase_config(&mut self) -> Result<(), Error<S::Error>> {
        self.map.remove_item(&mut self.buffer, &CONFIG_KEY).await
    }
}

#[cfg(test)]
mod tests;
