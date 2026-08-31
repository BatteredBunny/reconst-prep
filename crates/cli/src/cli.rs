//! Every clap type, and nothing else: a change here is a change to the published interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use reconst_prep_core::mask::SkyParams;
use reconst_prep_core::seg::SegClassParams;

const ABOUT: &str = "Reconstruction-ready image datasets from video in one pass:\n\
frames, optionally lens-corrected, resized, thinned to the ones worth\n\
keeping, and with the sky or people masked out.\n\n\
Everything is opt-in. Without flags this is a fast video-to-frames\n\
splitter. `reconst-prep gui` opens the graphical frontend.";

fn long_version() -> &'static str {
    // Leaked because clap wants 'static; runs once at startup.
    Box::leak(
        format!(
            "{}\ngyroflow-core rev: {}\nLicense: GPL-3.0-or-later\nSource: {}",
            reconst_prep_core::TOOL_VERSION,
            reconst_prep_core::GYROFLOW_CORE_REV,
            env!("CARGO_PKG_REPOSITORY"),
        )
        .into_boxed_str(),
    )
}

#[derive(Parser, Debug)]
#[command(
    name = "reconst-prep",
    bin_name = "reconst-prep",
    about = ABOUT,
    version,
    long_version = long_version(),
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub prep: PrepArgs,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Launch the graphical frontend. Same pipeline, with a live before and
    /// after preview.
    ///
    /// Everything here is optional. The window opens empty. Naming a job saves
    /// finding it again in a file dialog.
    Gui {
        /// Video files and/or directories to start with.
        inputs: Vec<PathBuf>,

        /// Gyroflow lens profile, which also switches undistortion on.
        #[arg(short, long)]
        profile: Option<PathBuf>,

        /// Output directory.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },

    /// Search and download Gyroflow lens profiles.
    ///
    /// This is the only part of the tool that touches the network, and only
    /// when you ask it to. A normal run never does.
    Profiles {
        #[command(subcommand)]
        what: ProfileCmd,
    },

    /// Print a shell completion script on stdout.
    Completions {
        /// bash, zsh, fish, elvish or powershell.
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProfileCmd {
    /// List profiles matching every given term, e.g. `dji o4 4k`.
    Search {
        /// Terms to match against the profile path. No terms lists everything.
        terms: Vec<String>,
        /// Use only the cached index. Never touch the network.
        #[arg(long)]
        offline: bool,
    },
    /// Download a profile by its repository path and print the local file.
    /// Feed that path straight to --profile.
    Get {
        /// Exact path from `profiles search`, e.g. "DJI/DJI_O4 Pro_....json".
        path: String,
    },
    /// Refresh the cached profile index.
    Update,
}

#[derive(clap::Args, Debug)]
pub struct PrepArgs {
    /// Video files and/or directories of video clips (one output dataset).
    /// Required unless a subcommand is given.
    pub inputs: Vec<PathBuf>,

    /// Gyroflow lens profile JSON for the camera. Supplying it turns ON
    /// fisheye/lens undistortion. Without it frames are written as decoded.
    #[arg(short, long)]
    pub profile: Option<PathBuf>,

    /// Output directory for the image dataset. Required unless a subcommand
    /// is given.
    #[arg(short, long)]
    pub out: Option<PathBuf>,

    /// Output size: "same", a scale factor like "0.5", or WxH like "1920x1080".
    /// A different aspect than the input crops the field of view exactly like
    /// a Gyroflow app export at that size.
    #[arg(long, default_value = "same")]
    pub size: String,

    /// Output image format: "png" or "jpeg".
    #[arg(long, default_value = "jpeg")]
    pub format: String,

    /// JPEG quality (1-100).
    #[arg(
        long,
        default_value_t = reconst_prep_core::output::DEFAULT_JPEG_QUALITY,
        value_parser = clap::value_parser!(u8).range(1..=100),
    )]
    pub jpeg_quality: u8,

    /// Selection mode: "motion" (default) or "every-nth".
    #[arg(long, default_value = "motion")]
    pub select: String,

    /// every-nth: keep every Nth frame.
    #[arg(long, default_value_t = reconst_prep_core::select::DEFAULT_NTH)]
    pub nth: u32,

    /// motion: how much the shot must change [0..1] before the next frame is
    /// kept. Lower keeps more frames; drone footage usually works at 0.02-0.10.
    #[arg(long, default_value_t = reconst_prep_core::select::DEFAULT_MOTION_THRESHOLD)]
    pub motion_threshold: f64,

    /// motion: how many frames to compare before choosing. The sharpest wins.
    #[arg(long, default_value_t = reconst_prep_core::select::DEFAULT_WINDOW)]
    pub window: u32,

    /// Discard frames whose sharpness score falls below this, regardless of
    /// mode. Higher is stricter; around 100 is a reasonable start.
    #[arg(long)]
    pub blur_floor: Option<f64>,

    /// Hardware decoding: auto | none | vulkan | nvdec | cuda | vaapi | qsv.
    /// If the chosen backend produces nothing, software decoding takes over.
    #[arg(long, default_value = "auto")]
    pub hwaccel: String,

    /// Explicit ffmpeg binary. The default resolves from PATH, never downloads.
    #[arg(long)]
    pub ffmpeg_path: Option<PathBuf>,

    /// Undistort interpolation: bilinear | bicubic | lanczos4.
    ///
    /// The default is the right choice when downscaling. Choose lanczos4 when
    /// rendering at or near native size, where its extra sharpness is visible;
    /// it runs at roughly half the speed.
    #[arg(long, default_value = "bilinear")]
    pub interpolation: String,

    /// Undistort on the GPU. On by default; falls back to the CPU
    /// automatically when no usable device is found.
    #[arg(long, overrides_with = "no_gpu")]
    pub gpu: bool,

    /// Undistort on the CPU even where a GPU would work.
    #[arg(long)]
    pub no_gpu: bool,

    /// Mask the sky out. Stops sky pixels counting towards the movement and
    /// sharpness metrics. Otherwise drifting clouds read as scene change and
    /// seed splat floaters. The sky goes into masks/ (structure-from-motion)
    /// but NOT into masks_train/: masking it in every frame leaves that
    /// region unsupervised in every view and measurably degrades training.
    #[arg(long)]
    pub mask_sky: bool,

    /// Sky heuristic: minimum luma for a pixel to be sky.
    #[arg(long, default_value_t = SkyParams::default().luma_min, value_name = "0-255")]
    pub sky_luma: u8,

    /// Sky heuristic: minimum blue-minus-red. Negative admits grey overcast.
    #[arg(long, default_value_t = SkyParams::default().blue_bias, allow_negative_numbers = true)]
    pub sky_blue: i16,

    /// Sky heuristic: the region fill stops at luma steps above this.
    #[arg(long, default_value_t = SkyParams::default().gradient_max)]
    pub sky_edge: u8,

    /// Grow the sky mask by this many pixels (at metric resolution). Sky
    /// edges are soft at that scale, which is where the heuristic errs.
    #[arg(long, default_value_t = SkyParams::default().dilate)]
    pub sky_dilate: u32,

    /// Mask out people. Needs --seg-model. People move between frames, so
    /// features landed on them are matched as static structure and poison the
    /// pose estimates, then come back as ghost geometry. Unlike the sky,
    /// people are masked in some frames only, so this class goes into BOTH
    /// masks/ and masks_train/. This is the case where masking a trainer is
    /// correct. Also the answer to "keep bystanders out of a published set".
    #[arg(long)]
    pub mask_people: bool,

    /// ONNX semantic-segmentation model, supplied by you. Never downloaded.
    /// An ADE20K-class model (SegFormer-B0 and friends) serves both --mask-sky
    /// and --mask-people. With it, --mask-sky uses the model instead of the
    /// brightness heuristic, which is what handles sunsets, fog and shots with
    /// no horizon in frame.
    #[arg(long, value_name = "MODEL.onnx")]
    pub seg_model: Option<PathBuf>,

    /// Segmentation inference width (height follows the aspect, both rounded
    /// up to a multiple of 32). The metric thumbnail is far too coarse for
    /// people: a bystander in 4K drone footage is tens of pixels tall.
    #[arg(long, default_value_t = reconst_prep_core::seg::DEFAULT_SEG_WIDTH)]
    pub seg_width: u32,

    /// Label id of "sky" in the model output. ADE20K by default. An extra
    /// background channel is detected and compensated automatically.
    #[arg(long, default_value_t = reconst_prep_core::seg::ADE20K_SKY)]
    pub seg_class_sky: usize,

    /// Label id of "person" in the model's output.
    #[arg(long, default_value_t = reconst_prep_core::seg::ADE20K_PERSON)]
    pub seg_class_people: usize,

    /// Grow the segmented sky mask by this many pixels, at inference size.
    #[arg(long, default_value_t = SegClassParams::sky().dilate)]
    pub seg_sky_dilate: u32,

    /// Grow the people mask by this many pixels. Larger than the sky default
    /// on purpose: people carry motion blur, and a mask that clips a moving
    /// limb leaves exactly the features the mask exists to remove.
    #[arg(long, default_value_t = SegClassParams::people().dilate)]
    pub seg_people_dilate: u32,

    /// Per-pixel temporal mode over this many consecutive frames (odd only, 1 =
    /// off). Per-frame segmentation flickers along the horizon and around a
    /// moving silhouette. A small window stops that.
    #[arg(long, default_value_t = reconst_prep_core::seg::DEFAULT_TEMPORAL_WINDOW)]
    pub seg_temporal_window: u32,

    /// Worker threads for encoding and writing images (0 = auto).
    #[arg(long, default_value_t = 0)]
    pub threads: usize,

    /// Continue an interrupted run: skip every clip already complete in --out.
    /// Completeness is per clip, not per frame, so a half-written clip is
    /// redone. Refuses outright if --out was produced with different settings,
    /// rather than mixing two selections into one dataset.
    #[arg(long)]
    pub resume: bool,

    /// Keep exactly the frames a previous run kept, read from its manifest,
    /// instead of computing a selection. This is how you re-emit a dataset at
    /// a different --size, --format or quality while keeping the filenames an
    /// existing reconstruction already references. Selection is NOT
    /// size-independent, so recomputing it would quietly change the frame set.
    #[arg(long, value_name = "MANIFEST.json", conflicts_with = "select")]
    pub frames_from: Option<PathBuf>,

    /// Suppress per-frame progress output.
    #[arg(short, long)]
    pub quiet: bool,
}
