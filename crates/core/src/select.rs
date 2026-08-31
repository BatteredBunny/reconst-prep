// Selection is NOT output-size independent: the thumbnail derives from the output frame, so near-ties inside a window flip.

use anyhow::{Result, anyhow, ensure};

use crate::gray::{GrayFrame, mean_abs_diff};

/// Selection configuration as chosen by the user.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SelectionConfig {
    pub mode: SelectionMode,
    /// Applied before selection in every mode. None = disabled.
    pub blur_floor: Option<f64>,
}

/// Selection-knob defaults, shared by the CLI flags and the GUI settings.
pub const DEFAULT_NTH: u32 = 15;
pub const DEFAULT_MOTION_THRESHOLD: f64 = 0.04;
pub const DEFAULT_WINDOW: u32 = 8;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum SelectionMode {
    EveryNth {
        n: u32,
    },
    MotionGated {
        /// Mean-abs-diff against the last kept frame, in [0,1]. Typical drone footage: 0.02-0.10.
        motion_threshold: f64,
        /// Frames in the candidate window; the sharpest one wins.
        window: u32,
    },
    /// No metric is consulted, so output size, format and quality cannot change the result.
    Replay {
        /// The manifest replayed, recorded in the new run's own manifest.
        source: String,
    },
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            mode: SelectionMode::MotionGated {
                motion_threshold: DEFAULT_MOTION_THRESHOLD,
                window: DEFAULT_WINDOW,
            },
            blur_floor: None,
        }
    }
}

/// Everything the selector sees for one frame.
pub struct FrameInfo {
    pub index: u64,
    pub sharpness: f64,
    pub gray: GrayFrame,
}

/// Decisions lag pushes while a window is open, so the pipeline buffers frames until theirs arrives.
#[derive(Debug, Clone)]
pub struct Decision {
    pub index: u64,
    pub keep: bool,
}

pub trait Selector: Send {
    /// Returns the decisions that became final: none, one, or a whole window at once.
    fn push(&mut self, frame: FrameInfo) -> Vec<Decision>;
    /// End of clip: resolve anything still pending.
    fn finish(&mut self) -> Result<Vec<Decision>>;
}

/// Per-clip inputs a selector may need beyond the user's configuration.
#[derive(Default)]
pub struct ClipSelection<'a> {
    /// Exact frame indices to keep, for replay only. Must be sorted.
    pub replay: Option<&'a [u64]>,
}

/// Build the selector for one clip.
pub fn make_selector(cfg: &SelectionConfig, clip: ClipSelection<'_>) -> Result<Box<dyn Selector>> {
    let inner: Box<dyn Selector> = match &cfg.mode {
        SelectionMode::EveryNth { n } => {
            ensure!(*n >= 1, "every-nth: n must be >= 1");
            Box::new(EveryNth {
                n: *n as u64,
                seen: 0,
            })
        }
        SelectionMode::Replay { source } => {
            let frames = clip
                .replay
                .ok_or_else(|| anyhow!("{source} has no recorded frame list for this clip"))?;
            // Deliberately not wrapped in BlurFloor: replay must not re-judge.
            return Ok(Box::new(Replay {
                frames: frames.to_vec(),
                pos: 0,
            }));
        }
        SelectionMode::MotionGated {
            motion_threshold,
            window,
        } => Box::new(MotionGated {
            threshold: *motion_threshold,
            window: (*window).max(1) as usize,
            last_kept: None,
            buf: Vec::new(),
        }),
    };
    Ok(match cfg.blur_floor {
        Some(floor) => Box::new(BlurFloor { floor, inner }),
        None => inner,
    })
}

/// Too-blurry frames never reach the inner selector.
struct BlurFloor {
    floor: f64,
    inner: Box<dyn Selector>,
}

impl Selector for BlurFloor {
    fn push(&mut self, frame: FrameInfo) -> Vec<Decision> {
        if frame.sharpness < self.floor {
            return vec![Decision {
                index: frame.index,
                keep: false,
            }];
        }
        self.inner.push(frame)
    }
    fn finish(&mut self) -> Result<Vec<Decision>> {
        self.inner.finish()
    }
}

/// Matched against the decoder's numbering, which is what the filenames carry, so a replay reproduces them.
struct Replay {
    frames: Vec<u64>,
    pos: usize,
}

impl Selector for Replay {
    fn push(&mut self, f: FrameInfo) -> Vec<Decision> {
        // Sorted list, in-order frames: a walk.
        while self.pos < self.frames.len() && self.frames[self.pos] < f.index {
            self.pos += 1;
        }
        let keep = self.frames.get(self.pos) == Some(&f.index);
        if keep {
            self.pos += 1;
        }
        vec![Decision {
            index: f.index,
            keep,
        }]
    }
    fn finish(&mut self) -> Result<Vec<Decision>> {
        // Recorded frames this clip never produced: the dataset a
        // reconstruction references by name would come out short of them.
        let missing = self.frames.len() - self.pos;
        ensure!(
            missing == 0,
            "replay: {missing} of {} recorded frames are not in this clip. \
             Is it the footage the manifest was made from?",
            self.frames.len()
        );
        Ok(vec![])
    }
}

struct EveryNth {
    n: u64,
    seen: u64,
}

impl Selector for EveryNth {
    fn push(&mut self, f: FrameInfo) -> Vec<Decision> {
        let keep = self.seen.is_multiple_of(self.n);
        self.seen += 1;
        vec![Decision {
            index: f.index,
            keep,
        }]
    }
    fn finish(&mut self) -> Result<Vec<Decision>> {
        Ok(vec![])
    }
}

struct MotionGated {
    threshold: f64,
    window: usize,
    last_kept: Option<GrayFrame>,
    /// Open candidate window (empty = gate closed).
    buf: Vec<FrameInfo>,
}

impl MotionGated {
    /// Resolve the open window: keep the sharpest frame, drop the rest.
    fn resolve(&mut self) -> Vec<Decision> {
        // No maximum = empty window = gate closed, nothing to resolve.
        let Some(best) = self
            .buf
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.sharpness.total_cmp(&b.1.sharpness))
            .map(|(i, _)| i)
        else {
            return vec![];
        };
        let mut out = Vec::with_capacity(self.buf.len());
        for (i, f) in std::mem::take(&mut self.buf).into_iter().enumerate() {
            let keep = i == best;
            if keep {
                self.last_kept = Some(f.gray.clone());
            }
            out.push(Decision {
                index: f.index,
                keep,
            });
        }
        out
    }
}

impl Selector for MotionGated {
    fn push(&mut self, f: FrameInfo) -> Vec<Decision> {
        if !self.buf.is_empty() {
            self.buf.push(f);
            if self.buf.len() >= self.window {
                return self.resolve();
            }
            return vec![];
        }
        let Some(last) = &self.last_kept else {
            // First (usable) frame of the clip anchors the sequence.
            self.last_kept = Some(f.gray.clone());
            return vec![Decision {
                index: f.index,
                keep: true,
            }];
        };
        let motion = mean_abs_diff(&f.gray, last);
        if motion >= self.threshold {
            self.buf.push(f);
            if self.buf.len() >= self.window {
                return self.resolve();
            }
            return vec![];
        }
        vec![Decision {
            index: f.index,
            keep: false,
        }]
    }

    fn finish(&mut self) -> Result<Vec<Decision>> {
        Ok(self.resolve())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gray::GrayFrame;

    fn gf(v: u8) -> GrayFrame {
        GrayFrame {
            w: 4,
            h: 4,
            data: vec![v; 16],
            valid: None,
        }
    }
    fn fi(index: u64, sharpness: f64, v: u8) -> FrameInfo {
        FrameInfo {
            index,
            sharpness,
            gray: gf(v),
        }
    }

    fn kept(decisions: &[Decision]) -> Vec<u64> {
        decisions
            .iter()
            .filter(|d| d.keep)
            .map(|d| d.index)
            .collect()
    }

    #[test]
    fn every_nth_keeps_every_nth() {
        let cfg = SelectionConfig {
            mode: SelectionMode::EveryNth { n: 3 },
            blur_floor: None,
        };
        let mut s = make_selector(&cfg, ClipSelection::default()).unwrap();
        let mut all = vec![];
        for i in 0..10 {
            all.extend(s.push(fi(i, 1.0, 0)));
        }
        all.extend(s.finish().unwrap());
        assert_eq!(kept(&all), vec![0, 3, 6, 9]);
    }

    #[test]
    fn motion_gated_keeps_sharpest_in_window() {
        let cfg = SelectionConfig {
            mode: SelectionMode::MotionGated {
                motion_threshold: 0.05,
                window: 3,
            },
            blur_floor: None,
        };
        let mut s = make_selector(&cfg, ClipSelection::default()).unwrap();
        let mut all = vec![];
        // Frame 0: anchor (kept). Frames 1-2: static (dropped).
        all.extend(s.push(fi(0, 1.0, 100)));
        all.extend(s.push(fi(1, 1.0, 100)));
        all.extend(s.push(fi(2, 1.0, 101)));
        // Frame 3: big change vs anchor opens a window; frame 4 is sharpest.
        all.extend(s.push(fi(3, 5.0, 200)));
        all.extend(s.push(fi(4, 9.0, 200)));
        all.extend(s.push(fi(5, 2.0, 200)));
        // Frame 6: static vs new anchor (frame 4) -> dropped.
        all.extend(s.push(fi(6, 1.0, 200)));
        all.extend(s.finish().unwrap());
        assert_eq!(kept(&all), vec![0, 4]);
        assert_eq!(all.len(), 7); // one decision per pushed frame
    }

    /// A shorter or re-encoded clip means the dataset a reconstruction
    /// references by name comes out short, which used to pass as a clean run.
    #[test]
    fn replay_refuses_a_clip_missing_recorded_frames() {
        let cfg = SelectionConfig {
            mode: SelectionMode::Replay {
                source: "prev.json".into(),
            },
            blur_floor: None,
        };
        let want = [0u64, 4, 40];
        let mut s = make_selector(
            &cfg,
            ClipSelection {
                replay: Some(&want),
            },
        )
        .unwrap();
        for i in 0..10 {
            s.push(fi(i, 1.0, 0));
        }
        let e = s.finish().expect_err("frame 40 is not in a 10-frame clip");
        assert!(e.to_string().contains("1 of 3"), "unhelpful: {e}");
    }

    #[test]
    fn replay_keeps_exactly_the_recorded_frames() {
        let cfg = SelectionConfig {
            mode: SelectionMode::Replay {
                source: "prev.json".into(),
            },
            blur_floor: None,
        };
        let want = [0u64, 4, 5, 9];
        let mut s = make_selector(
            &cfg,
            ClipSelection {
                replay: Some(&want),
            },
        )
        .unwrap();
        let mut all = vec![];
        for i in 0..10 {
            all.extend(s.push(fi(i, 1.0, 0)));
        }
        all.extend(s.finish().unwrap());
        assert_eq!(kept(&all), want);
        assert_eq!(all.len(), 10, "every frame gets exactly one decision");
    }

    #[test]
    fn replay_ignores_the_blur_floor() {
        // Re-judging sharpness would drop frames a reconstruction references.
        let cfg = SelectionConfig {
            mode: SelectionMode::Replay {
                source: "prev.json".into(),
            },
            blur_floor: Some(1000.0),
        };
        let want = [1u64, 2];
        let mut s = make_selector(
            &cfg,
            ClipSelection {
                replay: Some(&want),
            },
        )
        .unwrap();
        let mut all = vec![];
        for i in 0..4 {
            all.extend(s.push(fi(i, 0.1, 0))); // far below the floor
        }
        assert_eq!(kept(&all), want);
    }

    #[test]
    fn replay_without_a_frame_list_is_an_error() {
        let cfg = SelectionConfig {
            mode: SelectionMode::Replay {
                source: "prev.json".into(),
            },
            blur_floor: None,
        };
        assert!(make_selector(&cfg, ClipSelection::default()).is_err());
    }

    #[test]
    fn blur_floor_drops_regardless_of_mode() {
        let cfg = SelectionConfig {
            mode: SelectionMode::EveryNth { n: 1 },
            blur_floor: Some(10.0),
        };
        let mut s = make_selector(&cfg, ClipSelection::default()).unwrap();
        let mut all = vec![];
        all.extend(s.push(fi(0, 50.0, 0)));
        all.extend(s.push(fi(1, 5.0, 0))); // below floor
        all.extend(s.push(fi(2, 50.0, 0)));
        all.extend(s.finish().unwrap());
        assert_eq!(kept(&all), vec![0, 2]);
    }
}
