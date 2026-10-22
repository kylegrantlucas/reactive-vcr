//! Hot reload: reloads a mapping when it or one of its preset files changes.

use crate::mapping::{self, CompiledMapping};
use crate::params::ParamCatalog;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Minimum time between checks of the watched files.
pub const RELOAD_INTERVAL: Duration = Duration::from_millis(250);

/// Modification time, size, and file identity. Identity tells a file renamed over the path,
/// as atomic installs do, from the one it replaced even when both share a size and a coarse
/// timestamp.
type Stamp = Option<(SystemTime, u64, (u64, u64))>;

fn stamp(path: &Path) -> Stamp {
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.modified().ok()?,
        metadata.len(),
        identity(&metadata),
    ))
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Loads a mapping and notices edits to it and its presets, including replacement by rename.
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
        // Stamp before reading, so a replacement that lands mid-load is seen as a change.
        let map_stamp = stamp(&self.map);
        let map = mapping::load(&self.map, catalog)?;
        self.watched = std::iter::once((self.map.clone(), map_stamp))
            .chain(
                map.preset_inputs()
                    .into_iter()
                    .map(|(_, path)| (path.to_path_buf(), stamp(path))),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    // The same size, so only the content tells them apart.
    const RMS: &str = "[[modulate]]\ntarget = \"snow_intensity\"\nsource = \"rms\"  \n";
    const ONSET: &str = "[[modulate]]\ntarget = \"snow_intensity\"\nsource = \"onset\"\n";

    /// A control server installs mappings by writing a temporary file and renaming it over the
    /// watched path. The replacement must be noticed even when it has the same size and, on a
    /// filesystem with coarse timestamps, the same modification time.
    #[test]
    fn notices_a_mapping_replaced_by_rename() {
        assert_eq!(RMS.len(), ONSET.len());
        let dir = tempfile::tempdir().unwrap();
        let map = dir.path().join("map.toml");
        std::fs::write(&map, RMS).unwrap();
        let catalog = ParamCatalog::new();
        let mut watcher = MappingWatcher::new(map.clone());
        watcher.load(&catalog).unwrap();

        let temp = dir.path().join("map.toml.tmp");
        std::fs::write(&temp, ONSET).unwrap();
        let modified = std::fs::metadata(&map).unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&temp)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        std::fs::rename(&temp, &map).unwrap();

        watcher.last_check -= RELOAD_INTERVAL;
        let reloaded = watcher
            .poll(&catalog)
            .expect("replacement noticed")
            .unwrap();
        assert_eq!(
            reloaded.modulations[0].source,
            crate::mapping::Source::Onset
        );
    }
}
