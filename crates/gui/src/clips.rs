// The clip list and everything derived from it. One type so that adding another
// per-clip map cannot leave a caller updating only some of them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::native::images::ImageSource;

use reconst_prep_core::decode::ClipInfo;
use reconst_prep_core::frame::Size;

use crate::preview;

#[derive(Default)]
pub(crate) struct ClipLibrary {
    inputs: Vec<PathBuf>,
    /// Cached because expanding walks the filesystem and the UI asks several times per frame.
    expanded: Vec<PathBuf>,
    /// The `inputs` value `expanded` was computed from.
    expanded_from: Vec<PathBuf>,
    /// ffprobe results, filled in by a worker. `Err` for files that will fail.
    probes: HashMap<PathBuf, Result<ClipInfo, String>>,
    /// Separate from `probes`, where every entry is an answer.
    probes_pending: HashSet<PathBuf>,
    /// `None` means tried and failed, which is what stops it being retried every frame.
    thumbs: HashMap<PathBuf, Option<ImageSource>>,
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
        // Removing a folder must release its children's cached pixels too. Keep pending
        // workers accounted for until their replies arrive, but not their old results.
        let active: HashSet<_> = self.expanded.iter().collect();
        self.probes.retain(|path, _| active.contains(path));
        self.thumbs.retain(|path, _| active.contains(path));
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

    pub fn inputs(&self) -> &[PathBuf] {
        &self.inputs
    }

    pub fn probe(&self, path: &Path) -> Option<&Result<ClipInfo, String>> {
        self.probes.get(path)
    }

    pub fn thumb(&self, path: &Path) -> Option<&Option<ImageSource>> {
        self.thumbs.get(path)
    }

    /// Adds a path unless it is already listed; reports whether it was new.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let new = !self.inputs.contains(&path);
        if new {
            self.inputs.push(path);
        }
        new
    }

    /// Remove displayed/cached data while retaining accounting for workers still in flight.
    pub fn remove(&mut self, path: &Path) {
        self.inputs.retain(|i| i != path);
        self.probes.remove(path);
        // Workers still running count against the cap until their answers arrive.
        self.thumbs.remove(path);
    }

    pub fn clear(&mut self) {
        let running = self.thumbs_running;
        let probing = std::mem::take(&mut self.probes_pending);
        let thumbnails = std::mem::take(&mut self.thumbs_pending);
        *self = Self::default();
        // In-flight workers still report back, and their answers are dropped by
        // `on_thumbnail`/`on_probed`; what they hold must still count against the caps.
        self.thumbs_running = running;
        self.probes_pending = probing;
        self.thumbs_pending = thumbnails;
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
    pub fn source_ceiling(&self) -> Option<Size> {
        self.probed()
            .iter()
            .map(|(_, c)| c.size)
            .reduce(|a, b| Size::new(a.w.min(b.w), a.h.min(b.h)))
    }

    /// More than one means a run will fail partway.
    pub fn mixed_resolutions(&self) -> Vec<Size> {
        let mut sizes: Vec<Size> = self.probed().iter().map(|(_, c)| c.size).collect();
        sizes.sort_unstable_by_key(|s| (s.w, s.h));
        sizes.dedup();
        sizes
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

    pub fn on_thumbnail(&mut self, path: PathBuf, tex: Option<ImageSource>) {
        self.thumbs_running = self.thumbs_running.saturating_sub(1);
        self.thumbs_pending.remove(&path);
        if self.inputs.contains(&path) || self.expanded.contains(&path) {
            self.thumbs.insert(path, tex);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removing_a_folder_releases_its_child_caches() {
        let folder = PathBuf::from("footage");
        let child = folder.join("clip.mp4");
        let mut clips = ClipLibrary {
            inputs: vec![folder.clone()],
            expanded_from: vec![folder.clone()],
            expanded: vec![child.clone()],
            ..ClipLibrary::default()
        };
        clips.probes.insert(child.clone(), Err("unreadable".into()));
        clips.thumbs.insert(child, None);
        clips.remove(&folder);
        clips.refresh();
        assert!(clips.files().is_empty());
        assert!(clips.probes.is_empty());
        assert!(clips.thumbs.is_empty());
    }

    #[test]
    fn removing_and_readding_does_not_release_inflight_worker_slots() {
        let path = PathBuf::from("clip.mp4");
        let mut clips = ClipLibrary::default();
        clips.inputs.push(path.clone());
        clips.expanded.push(path.clone());
        assert_eq!(clips.claim_unprobed(1), Some(path.clone()));
        clips.thumbs_pending.insert(path.clone());
        clips.thumbs_running = 1;
        clips.remove(&path);
        clips.add(path.clone());
        assert!(clips.claim_unprobed(1).is_none());
        assert!(clips.thumbs_pending.contains(&path));
        assert_eq!(clips.thumbs_running, 1);
        clips.clear();
        clips.add(path.clone());
        assert!(clips.probes_pending.contains(&path));
        assert!(clips.thumbs_pending.contains(&path));
        clips.on_probed(path.clone(), Err("unreadable".into()));
        clips.on_thumbnail(path.clone(), None);
        assert!(!clips.probes_pending.contains(&path));
        assert!(!clips.thumbs_pending.contains(&path));
        assert_eq!(clips.thumbs_running, 0);
    }
}
