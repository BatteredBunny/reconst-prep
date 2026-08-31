// Frames come over a pipe, with no intermediate video file; ffmpeg is never auto-downloaded.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, bounded};
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;

/// A failed hwaccel init must not kill the run: an explicit backend that fails is retried without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwAccel {
    None,
    Auto,
    Backend(String),
}

impl HwAccel {
    /// Any other name still reaches ffmpeg; this only decides what is suggested and what passes without a warning.
    pub const KNOWN_BACKENDS: &'static [&'static str] =
        &["vulkan", "nvdec", "cuda", "vaapi", "qsv"];

    pub fn as_arg(&self) -> Option<&str> {
        match self {
            HwAccel::None => None,
            HwAccel::Auto => Some("auto"),
            HwAccel::Backend(b) => Some(b),
        }
    }
}

impl std::str::FromStr for HwAccel {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "none" | "off" => HwAccel::None,
            "auto" => HwAccel::Auto,
            other => HwAccel::Backend(other.to_string()),
        })
    }
}

impl std::fmt::Display for HwAccel {
    /// Reaches the manifest, so it must round-trip through `FromStr`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_arg().unwrap_or("none"))
    }
}

/// Resolved ffmpeg/ffprobe locations plus the version string for the manifest.
#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: Option<PathBuf>,
    pub version: String,
}

/// Only for looking in a known directory; `PATH` resolution needs no suffix on any platform.
fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

impl Ffmpeg {
    /// In order: an explicit path, a copy beside our own executable, then PATH. Never downloads.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self> {
        let ffmpeg: PathBuf = match explicit {
            Some(p) => {
                if !p.exists() {
                    bail!("--ffmpeg-path {} does not exist", p.display());
                }
                p.to_path_buf()
            }
            None => std::env::current_exe()
                .ok()
                .and_then(|exe| {
                    let sibling = exe.parent()?.join(exe_name("ffmpeg"));
                    sibling.is_file().then_some(sibling)
                })
                .unwrap_or_else(|| PathBuf::from("ffmpeg")),
        };
        let out = Command::new(&ffmpeg)
            .arg("-version")
            .output()
            .with_context(|| {
                format!(
                    "could not run {}. Install ffmpeg and put it on PATH, or pass --ffmpeg-path",
                    ffmpeg.display()
                )
            })?;
        if !out.status.success() {
            bail!("{} -version exited with {}", ffmpeg.display(), out.status);
        }
        let version = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();

        // ffprobe: same directory as the resolved ffmpeg, else PATH.
        let probe_name = exe_name("ffprobe");
        let candidate = if ffmpeg
            .parent()
            .map(|p| !p.as_os_str().is_empty())
            .unwrap_or(false)
        {
            ffmpeg.with_file_name(probe_name)
        } else {
            PathBuf::from(probe_name)
        };
        let ffprobe = Command::new(&candidate)
            .arg("-version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| candidate);

        Ok(Self {
            ffmpeg,
            ffprobe,
            version,
        })
    }
}

/// Per-clip metadata from ffprobe (with fallbacks when fields are missing).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipInfo {
    pub path: PathBuf,
    /// Prefix for output image names; de-duplicated by the pipeline.
    pub stem: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Best-effort total (nb_frames, else duration*fps), for progress only.
    pub frames: Option<u64>,
    /// Codec short name (`hevc`, `h264`, …), shown in the GUI clip list.
    pub codec: Option<String>,
    pub duration_s: Option<f64>,
}

pub fn probe_clip(ff: &Ffmpeg, path: &Path) -> Result<ClipInfo> {
    let Some(ffprobe) = &ff.ffprobe else {
        bail!(
            "ffprobe not found next to ffmpeg or on PATH; it ships with every ffmpeg distribution"
        );
    };
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,nb_frames,r_frame_rate,duration,codec_name",
            "-show_entries",
            "format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("running ffprobe on {}", path.display()))?;
    if !out.status.success() {
        bail!(
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).context("ffprobe json")?;
    let stream = v["streams"]
        .get(0)
        .with_context(|| format!("{} has no video stream", path.display()))?;
    let width = stream["width"].as_u64().unwrap_or(0) as u32;
    let height = stream["height"].as_u64().unwrap_or(0) as u32;
    if width == 0 || height == 0 {
        bail!("{}: could not determine video dimensions", path.display());
    }
    let fps = stream["r_frame_rate"]
        .as_str()
        .and_then(|s| {
            let (n, d) = s.split_once('/')?;
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            if d > 0.0 { Some(n / d) } else { None }
        })
        .unwrap_or(0.0);
    check_frame_rate(path, fps)?;
    let duration_s = stream["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            v["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse().ok())
        });
    let frames = stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            let dur = duration_s?;
            if fps > 0.0 {
                Some((dur * fps).round() as u64)
            } else {
                None
            }
        });
    let codec = stream["codec_name"].as_str().map(|s| s.to_string());
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    Ok(ClipInfo {
        path: path.to_path_buf(),
        stem,
        width,
        height,
        fps,
        frames,
        codec,
        duration_s,
    })
}

/// Where ffmpeg starts abbreviating its progress reports as "1k".
const ABBREVIATED_FPS: f64 = 1000.0;

/// ffmpeg reports `fps=1k`, ffmpeg-sidecar never parses it, so it never reads stdout and ffmpeg blocks forever on a full pipe.
fn check_frame_rate(path: &Path, fps: f64) -> Result<()> {
    if fps >= ABBREVIATED_FPS {
        bail!(
            "{}: the file reports {fps:.0} frames per second, which cannot be decoded. \
             Rates like this come from variable-frame-rate recordings (screen captures \
             especially). Re-encode to a fixed rate first, e.g. \
             `ffmpeg -i {} -fps_mode cfr -r 60 fixed.mp4`.",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

/// One decoded video frame, tightly packed RGB8.
pub struct RawFrame {
    pub index: u64,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Runs ffmpeg on a background thread over a bounded channel, kept shallow because 4K frames are ~33 MB.
pub struct Decoder {
    rx: Receiver<Result<RawFrame>>,
    /// Set to make the reader thread stop and reap ffmpeg.
    cancel: crate::cancel::CancelToken,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Decoder {
    pub fn spawn(ff: &Ffmpeg, path: &Path, hwaccel: &HwAccel) -> Result<Decoder> {
        let (tx, rx) = bounded::<Result<RawFrame>>(4);
        let cancel = crate::cancel::CancelToken::new();
        let c = cancel.clone();
        let ffmpeg = ff.ffmpeg.clone();
        let path = path.to_path_buf();
        let hw = hwaccel.clone();
        let thread = std::thread::Builder::new()
            .name("ffmpeg-decode".into())
            .spawn(move || {
                // A hwaccel that fails to init, or delivers no frames, warns once and re-decodes in software.
                let retry_in_software = |reason: String| {
                    eprintln!("warning: hwaccel '{hw}' {reason}; retrying with software decode");
                    if let Err(e) = run_ffmpeg(&ffmpeg, &path, None, &tx, &c).result {
                        let _ = tx.send(Err(e));
                    }
                };
                let cancelled = || c.is_cancelled();

                let run = run_ffmpeg(&ffmpeg, &path, hw.as_arg(), &tx, &c);
                match run.result {
                    Ok(()) if run.frames == 0 && !cancelled() => {
                        if hw.as_arg().is_some() {
                            retry_in_software(format!("produced no frames for {}", path.display()));
                        } else {
                            let _ = tx.send(Err(anyhow::anyhow!(
                                "ffmpeg produced no frames for {}",
                                path.display()
                            )));
                        }
                    }
                    Ok(()) => {}
                    Err(e) => {
                        // Only a run that delivered nothing can be retried: the frames already sent would arrive twice.
                        if hw.as_arg().is_some() && run.frames == 0 && !cancelled() {
                            retry_in_software(format!(
                                "decode failed for {} ({e:#})",
                                path.display()
                            ));
                        } else {
                            let _ = tx.send(Err(e));
                        }
                    }
                }
            })
            .context("spawning decode thread")?;
        Ok(Decoder {
            rx,
            cancel,
            thread: Some(thread),
        })
    }

    /// Blocking iterator over decoded frames. Ends at clip end or first error.
    pub fn frames(&self) -> impl Iterator<Item = Result<RawFrame>> + '_ {
        self.rx.iter()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.cancel();
        // Drain so the sender never blocks forever on a full channel.
        while self.rx.try_recv().is_ok() {}
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// `scale_w` makes ffmpeg scale first, cutting a 33 MB 4K frame to tens of kilobytes over the pipe.
pub fn decode_frame_at_scaled(
    ff: &Ffmpeg,
    path: &Path,
    hwaccel: &HwAccel,
    start_s: f64,
    scale_w: Option<u32>,
) -> Result<RawFrame> {
    let spawn = |hw: Option<&str>| -> Result<Option<RawFrame>> {
        let mut cmd = FfmpegCommand::new_with_path(&ff.ffmpeg);
        if let Some(hw) = hw {
            cmd.hwaccel(hw);
        }
        if scale_w.is_some() {
            // A thumbnail can snap to the seek's keyframe; the full-size preview cannot, its arrows step one frame.
            cmd.arg("-noaccurate_seek");
            // Frame-parallel decode cannot help one frame; 2 threads match the default 24 at 40% of the CPU.
            cmd.args(["-threads", "2"]);
        }
        cmd.seek(format!("{start_s:.3}"));
        cmd.arg("-i").arg(path);
        cmd.frames(1);
        if let Some(w) = scale_w {
            // fast_bilinear: a thumbnail cannot tell the difference, and bicubic is a real share of the work at 11 megapixels.
            cmd.filter(format!("scale={w}:-2:flags=fast_bilinear"));
        }
        cmd.no_audio().rawvideo();
        let mut child = cmd.spawn().context("spawning ffmpeg")?;
        let iter = child.iter().context("attaching to ffmpeg output")?;
        let mut frame = None;
        for event in iter {
            if let FfmpegEvent::OutputFrame(f) = event
                && frame.is_none()
            {
                frame = Some(RawFrame {
                    index: 0,
                    width: f.width,
                    height: f.height,
                    rgb: f.data,
                });
            }
        }
        let _ = child.wait();
        Ok(frame)
    };
    // Same software-retry policy as `Decoder::spawn`, but silent: a slower preview is not worth a warning.
    let frame = match spawn(hwaccel.as_arg())? {
        None if hwaccel.as_arg().is_some() => spawn(None)?,
        first => first,
    };
    frame.with_context(|| format!("no frame decoded from {}", path.display()))
}

/// The frame count matters even when the run failed: a partial decode cannot be retried, the caller holds those frames.
struct FfmpegRun {
    frames: u64,
    /// A cancel or a receiver hangup counts as a clean end.
    result: Result<()>,
}

fn run_ffmpeg(
    ffmpeg: &Path,
    input: &Path,
    hwaccel: Option<&str>,
    tx: &crossbeam_channel::Sender<Result<RawFrame>>,
    cancel: &crate::cancel::CancelToken,
) -> FfmpegRun {
    let mut frames: u64 = 0;
    let result = (|| -> Result<()> {
        let mut cmd = FfmpegCommand::new_with_path(ffmpeg);
        if let Some(hw) = hwaccel {
            cmd.hwaccel(hw);
        }
        // `-i` by hand rather than `input()`, which takes a `&str` and would
        // force a lossy conversion: a non-UTF-8 name probes fine then fails here.
        cmd.arg("-i").arg(input);
        // Tightly-packed RGB8, stride = w*3, as gyroflow-core consumes it.
        cmd.rawvideo();
        let mut child = cmd.spawn().context("spawning ffmpeg")?;
        let iter = child.iter().context("attaching to ffmpeg output")?;

        let mut err: Option<String> = None;
        for event in iter {
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            match event {
                FfmpegEvent::OutputFrame(f) => {
                    frames += 1;
                    let frame = RawFrame {
                        index: frames - 1,
                        width: f.width,
                        height: f.height,
                        rgb: f.data,
                    };
                    if tx.send(Ok(frame)).is_err() {
                        // Pipeline stopped; reap ffmpeg.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(());
                    }
                }
                FfmpegEvent::Error(e) => err = Some(e),
                _ => {}
            }
        }
        let status = child.wait().context("waiting for ffmpeg")?;
        // Both legitimate early stops returned above, so a failed status here means a truncated clip.
        if !status.success() {
            bail!(
                "ffmpeg failed on {} after {frames} frames: {}",
                input.display(),
                // `ExitStatus`'s own Display already says "exit status: N".
                err.unwrap_or_else(|| status.to_string())
            );
        }
        Ok(())
    })();
    FfmpegRun { frames, result }
}

/// Serialized as the `-hwaccel` name, so a manifest records what was passed and
/// an unknown backend survives the round trip rather than being rewritten.
impl serde::Serialize for HwAccel {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for HwAccel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = <std::borrow::Cow<'_, str>>::deserialize(d)?;
        name.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absurd_frame_rates_are_refused_by_name() {
        let path = Path::new("/tmp/screencap.mp4");
        assert!(check_frame_rate(path, 59.94).is_ok());
        assert!(check_frame_rate(path, 999.0).is_ok());
        let e = check_frame_rate(path, 2000.0)
            .expect_err("1k+ fps deadlocks the decoder, so it must be refused")
            .to_string();
        assert!(e.contains("screencap.mp4"), "must name the file: {e}");
        assert!(
            e.contains("2000 frames per second"),
            "must name the rate: {e}"
        );
    }
}
