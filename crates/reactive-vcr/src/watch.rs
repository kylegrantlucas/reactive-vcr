//! Hot reload: reloads a mapping when it or one of its preset files changes.

use crate::mapping::{self, CompiledMapping};
use crate::params::ParamCatalog;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Minimum time between checks of the watched files.
pub const RELOAD_INTERVAL: Duration = Duration::from_millis(250);

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Loads a mapping and notices edits to it and its presets by modification time and size.
pub struct MappingWatcher {
    map: PathBuf,
    watched: Vec<(PathBuf, Stamp)>,
    last_check: Instant,
}

impl MappingWatcher {
    /// Starts by watching only `map`, so an edit retries even if the first load fails.
    pub fn new(map: PathBuf) -> Self {
        let watched = vec![(map.clone(), stamp(&map))];
        Self {
            map,
            watched,
            last_check: Instant::now(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.map
    }

    /// Load the mapping; on success, watch it plus its presets. A failed load keeps watching
    /// the previous files.
    pub fn load(&mut self, catalog: &ParamCatalog) -> Result<CompiledMapping> {
        let map = mapping::load(&self.map, catalog)?;
        self.watched = std::iter::once(self.map.as_path())
            .chain(map.preset_inputs().into_iter().map(|(_, path)| path))
            .map(|path| (path.to_path_buf(), stamp(path)))
            .collect();
        Ok(map)
    }

    /// The reloaded mapping once any watched file changes, checked at most every
    /// [`RELOAD_INTERVAL`]. A failed load is reported once and retried after the next change.
    pub fn poll(&mut self, catalog: &ParamCatalog) -> Option<Result<CompiledMapping>> {
        if self.last_check.elapsed() < RELOAD_INTERVAL {
            return None;
        }
        self.last_check = Instant::now();
        let mut changed = false;
        for (path, seen) in &mut self.watched {
            let current = stamp(path);
            if current != *seen {
                *seen = current;
                changed = true;
            }
        }
        changed.then(|| self.load(catalog))
    }
}
