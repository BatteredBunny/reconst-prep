use anyhow::{Result, bail};

use reconst_prep_core::decode::HwAccel;
use reconst_prep_core::mask::{MaskConfig, MaskSourceError, MaskSources, SkyParams};
use reconst_prep_core::output::ImageFormat;
use reconst_prep_core::pipeline::PipelineConfig;
use reconst_prep_core::seg::SegClassParams;
use reconst_prep_core::select::{SelectionConfig, SelectionMode};

use crate::cli::{FormatArg, PrepArgs, SelectArg};

pub fn build_config(args: &PrepArgs) -> Result<PipelineConfig> {
    if args.inputs.is_empty() {
        bail!("no input videos or directories given (see --help)");
    }
    let Some(out) = args.out.clone() else {
        bail!("--out <DIR> is required");
    };
    warn_unknown_hwaccel(&args.hwaccel);
    Ok(PipelineConfig {
        inputs: args.inputs.clone(),
        profile_path: args.profile.clone(),
        out_dir: out,
        output_size: args.size,
        format: match args.format {
            FormatArg::Png => ImageFormat::Png,
            FormatArg::Jpeg => ImageFormat::Jpeg {
                quality: args.jpeg_quality,
            },
        },
        selection: SelectionConfig {
            mode: selection_mode(args),
            blur_floor: args.blur_floor,
        },
        hwaccel: args.hwaccel.clone(),
        ffmpeg_path: args.ffmpeg_path.clone(),
        interp: args.interpolation,
        gpu: !args.no_gpu,
        mask: mask_config(args)?,
        // The writer pool is the only thread count worth exposing.
        writer_threads: args.threads,
        undistort_threads: 0,
        resume: args.resume,
        frames_from: args.frames_from.clone(),
    })
}

/// Anything else still reaches ffmpeg, but a typo is silently ignored there and costs a whole software decode.
fn warn_unknown_hwaccel(hw: &HwAccel) {
    if let HwAccel::Backend(b) = hw
        && !HwAccel::KNOWN_BACKENDS.contains(&b.as_str())
    {
        log::warn!("--hwaccel '{b}' is not a known backend; passing it to ffmpeg anyway");
    }
}

fn selection_mode(args: &PrepArgs) -> SelectionMode {
    match (&args.frames_from, args.select) {
        // A recorded frame list overrides selection; clap already refuses --select alongside it.
        (Some(path), _) => SelectionMode::Replay {
            source: path.display().to_string(),
        },
        (None, SelectArg::Motion) => SelectionMode::MotionGated {
            motion_threshold: args.motion_threshold,
            window: args.window,
        },
        (None, SelectArg::EveryNth) => SelectionMode::EveryNth { n: args.nth },
    }
}

fn mask_config(args: &PrepArgs) -> Result<MaskConfig> {
    MaskConfig::from_sources(MaskSources {
        mask_sky: args.mask_sky,
        sky_heuristic: SkyParams {
            luma_min: args.sky_luma,
            blue_bias: args.sky_blue,
            gradient_max: args.sky_edge,
            dilate: args.sky_dilate,
        },
        model: args.seg_model.clone(),
        sky_class: SegClassParams {
            class_id: args.seg_class_sky,
            dilate: args.seg_sky_dilate,
        },
        people_class: args.mask_people.then_some(SegClassParams {
            class_id: args.seg_class_people,
            dilate: args.seg_people_dilate,
        }),
        seg_width: args.seg_width,
        temporal_window: args.seg_temporal_window,
    })
    .map_err(|e| match e {
        MaskSourceError::PeopleWithoutModel => anyhow::anyhow!(
            "--mask-people needs --seg-model <MODEL.onnx>: people are found by segmentation, \
             and model weights are never downloaded for you"
        ),
    })
}
