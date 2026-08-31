use anyhow::{Result, bail};

use reconst_prep_core::decode::HwAccel;
use reconst_prep_core::mask::{MaskConfig, MaskSources, SkyParams};
use reconst_prep_core::output::ImageFormat;
use reconst_prep_core::pipeline::{MAX_SCALE, OutputSpec, PipelineConfig};
use reconst_prep_core::seg::SegClassParams;
use reconst_prep_core::select::{SelectionConfig, SelectionMode};
use reconst_prep_core::undistort::Interp;

use crate::cli::PrepArgs;

pub fn build_config(args: &PrepArgs) -> Result<PipelineConfig> {
    if args.inputs.is_empty() {
        bail!("no input videos or directories given (see --help)");
    }
    let Some(out) = args.out.clone() else {
        bail!("--out <DIR> is required");
    };
    // Order is deliberate: with several bad arguments the message names the same one it always has.
    let format = parse_format(args)?;
    let mode = selection_mode(args)?;
    let mask = mask_config(args)?;
    Ok(PipelineConfig {
        inputs: args.inputs.clone(),
        profile_path: args.profile.clone(),
        out_dir: out,
        output_size: parse_size(&args.size)?,
        format,
        selection: SelectionConfig {
            mode,
            blur_floor: args.blur_floor,
        },
        hwaccel: parse_hwaccel(&args.hwaccel),
        ffmpeg_path: args.ffmpeg_path.clone(),
        interp: args.interpolation.parse::<Interp>()?,
        gpu: !args.no_gpu,
        mask,
        // The writer pool is the only thread count worth exposing.
        writer_threads: args.threads,
        undistort_threads: 0,
        resume: args.resume,
        frames_from: args.frames_from.clone(),
    })
}

fn parse_size(s: &str) -> Result<OutputSpec> {
    let s = s.trim().to_ascii_lowercase();
    let bad = || {
        anyhow::anyhow!("--size must be \"same\", a scale factor like 0.5, or WxH like 1920x1080")
    };
    if s == "same" {
        return Ok(OutputSpec::Same);
    }
    if let Some((w, h)) = s.split_once('x') {
        return Ok(OutputSpec::Exact {
            width: w.parse().map_err(|_| bad())?,
            height: h.parse().map_err(|_| bad())?,
        });
    }
    let factor: f64 = s.parse().map_err(|_| bad())?;
    // Checked here rather than in `OutputSpec::resolve`, which runs only after a probe: a bad number is an argument error.
    if !(factor > 0.0 && factor <= MAX_SCALE) {
        bail!("--size scale factor must be above 0 and at most {MAX_SCALE}, not {factor}");
    }
    Ok(OutputSpec::Scale { factor })
}

/// Anything else still reaches ffmpeg, but a typo is silently ignored there and costs a whole software decode.
fn parse_hwaccel(s: &str) -> HwAccel {
    let hw = s.parse::<HwAccel>().unwrap_or(HwAccel::Auto);
    if let HwAccel::Backend(b) = &hw
        && !HwAccel::KNOWN_BACKENDS.contains(&b.as_str())
    {
        eprintln!("warning: --hwaccel '{b}' is not a known backend; passing it to ffmpeg anyway");
    }
    hw
}

fn parse_format(args: &PrepArgs) -> Result<ImageFormat> {
    Ok(match args.format.to_ascii_lowercase().as_str() {
        "png" => ImageFormat::Png,
        "jpeg" | "jpg" => ImageFormat::Jpeg {
            quality: args.jpeg_quality,
        },
        other => bail!("unknown format {other:?} (png|jpeg)"),
    })
}

fn selection_mode(args: &PrepArgs) -> Result<SelectionMode> {
    Ok(match &args.frames_from {
        // A recorded frame list overrides selection; clap already refuses --select alongside it.
        Some(path) => SelectionMode::Replay {
            source: path.display().to_string(),
        },
        None => match args.select.to_ascii_lowercase().as_str() {
            "motion" | "motion-gated" => SelectionMode::MotionGated {
                motion_threshold: args.motion_threshold,
                window: args.window,
            },
            "every-nth" | "nth" => SelectionMode::EveryNth { n: args.nth },
            other => bail!("unknown selection mode {other:?} (motion|every-nth)"),
        },
    })
}

fn mask_config(args: &PrepArgs) -> Result<MaskConfig> {
    if args.mask_people && args.seg_model.is_none() {
        bail!(
            "--mask-people needs --seg-model <MODEL.onnx>: people are found by segmentation, \
             and model weights are never downloaded for you"
        );
    }
    Ok(MaskConfig::from_sources(MaskSources {
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
    }))
}
