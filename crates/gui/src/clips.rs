// The clip list and everything derived from it. One type so that adding another
// per-clip map cannot leave a caller updating only some of them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui;

use reconst_prep_core::decode::ClipInfo;

use crate::preview;

#[derive(Default)]
pub(crate) struct ClipLibrary {
    /// What the user chose: files and directories, not yet expanded.
    pub inputs: Vec<PathBuf>,
    /// Cached because expanding walks the filesystem and the UI asks several times per frame.
    expanded: Vec<PathBuf>,
    /// The `inputs` value `expanded` was computed from.
    expanded_from: Vec<PathBuf>,
    /// ffprobe results, filled in by a worker. `Err` for files that will fail.
    pub probes: HashMap<PathBuf, Result<ClipInfo, String>>,
    /// Separate from `probes`, where every entry is an answer.
    probes_pending: HashSet<PathBuf>,
    /// `None` means tried and failed, which is what stops it being retried every frame.
    pub thumbs: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// Capped so a dropped folder of two hundred clips does not spawn a thread each.
    thumbs_running: usize,
    /// Separate from `thumbs`, where `None` means "tried and failed".
    thumbs_pending: HashSet<PathBuf>,
}

impl ClipLibrary {
    /// Once per frame, not per lookup: the expansion reads directories.
    pub fn refresh(&mut self) {
        if self.inputs == self.expanded_from {
            return;
        }
        self.expanded_from = self.inputs.clone();
        let before = self.expanded.len();
        self.expanded =
            reconst_prep_core::pipeline::expand_inputs(&self.inputs).unwrap_or_default();
        if self.expanded.len() != before {
            log::info!(
                "input list: {} clip{} from {} path{}",
                self.expanded.len(),
                crate::plural(self.expanded.len()),
                self.inputs.len(),
                crate::plural(self.inputs.len())
            );
        }
    }

    /// The clip files, with directories expanded.
    pub fn files(&self) -> &[PathBuf] {
        &self.expanded
    }

    /// Adds a path unless it is already listed; reports whether it was new.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let new = !self.inputs.contains(&path);
        if new {
            self.inputs.push(path);
        }
        new
    }

    /// Everything about one clip, so a removal cannot leave a stale map behind.
    pub fn remove(&mut self, path: &Path) {
        self.inputs.retain(|i| i != path);
        self.probes.remove(path);
        self.probes_pending.remove(path);
        self.thumbs.remove(path);
        self.thumbs_pending.remove(path);
    }

    pub fn clear(&mut self) {
        let running = self.thumbs_running;
        *self = Self::default();
        // In-flight workers still report back, and their answers are dropped
        // by `on_thumbnail`; the running count must survive to stay balanced.
        self.thumbs_running = running;
    }

    /// Probed info for every clip, in run order, skipping ones that failed.
    pub fn probed(&self) -> Vec<(&PathBuf, &ClipInfo)> {
        self.files()
            .iter()
            .filter_map(|p| {
                let info = self.probes.get(p)?.as_ref().ok()?;
                Some((p, info))
            })
            .collect()
    }

    /// The smallest probed frame size per axis; `None` while nothing has probed yet.
    pub fn source_ceiling(&self) -> Option<(u32, u32)> {
        self.probed()
            .iter()
            .map(|(_, c)| (c.width, c.height))
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
    }

    /// More than one means a run will fail partway.
    pub fn mixed_resolutions(&self) -> Vec<(u32, u32)> {
        let mut sizes: Vec<(u32, u32)> = self
            .probed()
            .iter()
            .map(|(_, c)| (c.width, c.height))
            .collect();
        sizes.sort_unstable();
        sizes.dedup();
        sizes
    }

    /// Is this clip's probe still in flight?
    pub fn probing(&self, path: &Path) -> bool {
        self.probes_pending.contains(path)
    }

    /// The next clip with no probe, claimed so the same file is not probed twice.
    pub fn claim_unprobed(&mut self, in_flight_cap: usize) -> Option<PathBuf> {
        if self.probes_pending.len() >= in_flight_cap {
            return None;
        }
        let path = self
            .expanded
            .iter()
            .find(|p| !self.probes.contains_key(*p) && !self.probes_pending.contains(*p))
            .cloned()?;
        self.probes_pending.insert(path.clone());
        Some(path)
    }

    /// The next probed clip with no thumbnail, and how long it runs. The probe
    /// carries the duration; re-probing in the worker would double the launches.
    pub fn claim_unthumbed(&mut self, in_flight_cap: usize) -> Option<(PathBuf, f64)> {
        if self.thumbs_running >= in_flight_cap {
            return None;
        }
        let (path, duration_s) = self.expanded.iter().find_map(|p| {
            if self.thumbs.contains_key(p) || self.thumbs_pending.contains(p) {
                return None;
            }
            let info = self.probes.get(p)?.as_ref().ok()?;
            Some((p.clone(), preview::duration_of(info).unwrap_or(0.0)))
        })?;
        self.thumbs_pending.insert(path.clone());
        self.thumbs_running += 1;
        Some((path, duration_s))
    }

    /// A worker's answer. Dropped when the clip has since been removed, so a
    /// probe in flight cannot resurrect an entry the user cleared.
    pub fn on_probed(&mut self, path: PathBuf, info: Result<ClipInfo, String>) {
        self.probes_pending.remove(&path);
        if self.inputs.contains(&path) || self.expanded.contains(&path) {
            self.probes.insert(path, info);
        }
    }

    pub fn on_thumbnail(&mut self, path: PathBuf, tex: Option<egui::TextureHandle>) {
        self.thumbs_running = self.thumbs_running.saturating_sub(1);
        self.thumbs_pending.remove(&path);
        if self.inputs.contains(&path) || self.expanded.contains(&path) {
            self.thumbs.insert(path, tex);
        }
    }
}
