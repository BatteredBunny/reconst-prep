use reconst_prep_core::undistort::{Interp, LensOnlyParams, undistort_rgb_image};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

const PATCH: usize = 192;
const GRID_X: usize = 10;
const GRID_Y: usize = 6;
const SIGMA: f64 = 4.0;

// Tolerances (spike-measured values in parentheses):
const MAX_MEDIAN_DISPLACEMENT_PX: f64 = 0.15; // (0.077)
const MAX_CORNER_SCALE_ERROR_PX: f64 = 0.5; // (0.083)
const MIN_PSNR_DB: f64 = 23.0; // (25.67; a 1-frame misalignment scores ~21)
// Set just under the 60 this fixture actually yields, not at half of it: a
// break that blanks part of the frame shows up as patches going low-texture.
const MIN_VALID_PATCHES: usize = 55; // (60 of 60 on the vendored frame)
// Both bounds come from the measurement, not the median's scale: this fixture's bottom-centre patches sit an order above the rest.
const MAX_P90_DISPLACEMENT_PX: f64 = 0.75; // (0.470)
const MAX_DISPLACEMENT_PX: f64 = 3.0; // (2.275, one patch at the bottom edge)

struct Gray {
    w: usize,
    h: usize,
    data: Vec<f64>,
}

fn load_gray(path: &str) -> Gray {
    let img = image::open(path).expect(path).to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    // PIL convert('L') luma, as the spike's analysis scripts used.
    let data = img
        .pixels()
        .map(|p| (299.0 * p.0[0] as f64 + 587.0 * p.0[1] as f64 + 114.0 * p.0[2] as f64) / 1000.0)
        .collect();
    Gray { w, h, data }
}

fn rgb_to_gray(rgb: &[u8], w: usize, h: usize) -> Gray {
    let data = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (299.0 * p[0] as f64 + 587.0 * p[1] as f64 + 114.0 * p[2] as f64) / 1000.0)
        .collect();
    Gray { w, h, data }
}

/// Separable gaussian blur, reflect boundary and 4-sigma truncation, both as scipy does them.
fn gaussian_blur(g: &Gray, sigma: f64) -> Gray {
    let radius = (4.0 * sigma + 0.5) as i64;
    let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
    for i in -radius..=radius {
        kernel.push((-0.5 * (i as f64 / sigma).powi(2)).exp());
    }
    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    let reflect = |i: i64, n: i64| -> usize {
        // scipy 'reflect': (d c b a | a b c d | d c b a)
        let mut i = i;
        loop {
            if i < 0 {
                i = -i - 1;
            } else if i >= n {
                i = 2 * n - i - 1;
            } else {
                return i as usize;
            }
        }
    };
    let (w, h) = (g.w, g.h);
    let mut tmp = vec![0.0f64; w * h];
    for y in 0..h {
        let row = &g.data[y * w..(y + 1) * w];
        for x in 0..w {
            let mut acc = 0.0;
            for (ki, k) in kernel.iter().enumerate() {
                let xi = reflect(x as i64 + ki as i64 - radius, w as i64);
                acc += row[xi] * k;
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (ki, k) in kernel.iter().enumerate() {
                let yi = reflect(y as i64 + ki as i64 - radius, h as i64);
                acc += tmp[yi * w + x] * k;
            }
            out[y * w + x] = acc;
        }
    }
    Gray { w, h, data: out }
}

fn fft2(data: &mut [Complex<f64>], n: usize, inverse: bool) {
    thread_local! {
        // FftPlanner memoizes per size, so one alive avoids re-planning.
        static PLANNER: std::cell::RefCell<FftPlanner<f64>> =
            std::cell::RefCell::new(FftPlanner::new());
    }
    let fft = PLANNER.with(|p| {
        let mut p = p.borrow_mut();
        if inverse {
            p.plan_fft_inverse(n)
        } else {
            p.plan_fft_forward(n)
        }
    });
    for row in data.chunks_exact_mut(n) {
        fft.process(row);
    }
    // Columns via transpose, rows, transpose back.
    let mut t = vec![Complex::new(0.0, 0.0); n * n];
    for y in 0..n {
        for x in 0..n {
            t[x * n + y] = data[y * n + x];
        }
    }
    for row in t.chunks_exact_mut(n) {
        fft.process(row);
    }
    for y in 0..n {
        for x in 0..n {
            data[y * n + x] = t[x * n + y];
        }
    }
}

/// Sub-pixel phase correlation of two same-size patches -> (dx, dy).
fn phase_corr(a: &[f64], b: &[f64], n: usize) -> (f64, f64) {
    let hann: Vec<f64> = (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos())
        .collect();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let windowed = |v: &[f64], m: f64| -> Vec<Complex<f64>> {
        let mut out = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                out.push(Complex::new((v[y * n + x] - m) * hann[y] * hann[x], 0.0));
            }
        }
        out
    };
    let mut fa = windowed(a, ma);
    let mut fb = windowed(b, mb);
    fft2(&mut fa, n, false);
    fft2(&mut fb, n, false);
    let mut r: Vec<Complex<f64>> = fa
        .iter()
        .zip(&fb)
        .map(|(x, y)| {
            let v = x * y.conj();
            let m = v.norm();
            v / if m == 0.0 { 1.0 } else { m }
        })
        .collect();
    fft2(&mut r, n, true);
    let c: Vec<f64> = r.iter().map(|v| v.re).collect();
    let peak = c
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap();
    let (py, px) = (peak / n, peak % n);
    // Parabolic sub-pixel refinement per axis, with wraparound.
    let subpix = |k: usize, get: &dyn Fn(usize) -> f64| -> f64 {
        let nn = n;
        let vm = get((k + nn - 1) % nn);
        let v0 = get(k);
        let vp = get((k + 1) % nn);
        let den = vm - 2.0 * v0 + vp;
        let mut s = k as f64
            + if den == 0.0 {
                0.0
            } else {
                0.5 * (vm - vp) / den
            };
        if s > nn as f64 / 2.0 {
            s -= nn as f64;
        }
        s
    };
    let sy = subpix(py, &|k| c[k * n + px]);
    let sx = subpix(px, &|k| c[py * n + k]);
    (sx, sy)
}

fn std_dev(v: &[f64]) -> f64 {
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

fn patch(g: &Gray, x0: usize, y0: usize, n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n * n);
    for y in 0..n {
        out.extend_from_slice(&g.data[(y0 + y) * g.w + x0..(y0 + y) * g.w + x0 + n]);
    }
    out
}

fn psnr(a: &Gray, b: &Gray) -> f64 {
    let mse = a
        .data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        / a.data.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

/// Loads the vendored lens profile, source frame, and app-export reference.
fn fixture() -> (LensOnlyParams, image::RgbImage, Gray) {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
    let profile_json =
        std::fs::read_to_string(format!("{data}/dji_o4_pro_stock_white_4k60.json")).unwrap();
    let src = image::open(format!("{data}/golden_src_frame200.png"))
        .unwrap()
        .to_rgb8();
    let (in_w, in_h) = (src.width() as usize, src.height() as usize);
    assert_eq!((in_w, in_h), (3840, 2880), "vendored source frame changed?");

    let params = LensOnlyParams::new(&profile_json, in_w, in_h, 1920, 1080).expect("profile");

    let reference = load_gray(&format!("{data}/golden_ref_frame200.png"));
    assert_eq!((reference.w, reference.h), (1920, 1080));
    (params, src, reference)
}

#[test]
fn golden_frame_matches_gyroflow_app_export() {
    let (params, src, reference) = fixture();

    // None here means someone bypassed LensProfile::from_json, so init() never ran.
    assert!(
        params.lens.fisheye_params.radial_distortion_limit.is_some(),
        "radial_distortion_limit not computed because LensProfile::init() did not run"
    );

    let out = undistort_rgb_image(&params, Interp::Lanczos4, src.as_raw()).expect("undistort");
    let ours = rgb_to_gray(&out, 1920, 1080);

    // The spike measured 25.7 dB; a single-frame misalignment scores ~21 dB and wrong lens math far less.
    let psnr_db = psnr(&ours, &reference);
    println!("psnr: {psnr_db:.2} dB");
    assert!(
        psnr_db > MIN_PSNR_DB,
        "structural mismatch: PSNR {psnr_db:.2} dB < {MIN_PSNR_DB} dB"
    );

    let r = register(&ours, &reference);
    println!(
        "patches: {} ({} low-texture, {} spurious)  \
         displacement median {:.4} px  mean {:.4} px  p90 {:.4} px  max {:.4} px",
        r.patches, r.low_texture, r.spurious, r.median, r.mean, r.p90, r.max
    );
    assert!(
        r.patches >= MIN_VALID_PATCHES,
        "only {} valid patches out of {} ({} low-texture, {} spurious), \
         so the images do not correlate",
        r.patches,
        GRID_X * GRID_Y,
        r.low_texture,
        r.spurious
    );
    assert_eq!(
        r.spurious, 0,
        "{} patches landed more than 5 px out. Skipping them would leave the \
         median measuring only the patches that still agree.",
        r.spurious
    );
    assert!(
        r.median < MAX_MEDIAN_DISPLACEMENT_PX,
        "median displacement {:.4} px exceeds {MAX_MEDIAN_DISPLACEMENT_PX} px. \
         Geometry no longer matches the Gyroflow app export.",
        r.median
    );
    assert!(
        r.p90 < MAX_P90_DISPLACEMENT_PX,
        "p90 displacement {:.4} px exceeds {MAX_P90_DISPLACEMENT_PX} px while \
         the median holds at {:.4} px: part of the frame regressed.",
        r.p90,
        r.median
    );
    assert!(
        r.max < MAX_DISPLACEMENT_PX,
        "worst patch is {:.4} px out, over {MAX_DISPLACEMENT_PX} px.",
        r.max
    );

    println!(
        "radial scale error {:.6} ({:.3} px at corner)",
        r.scale, r.corner_px
    );
    assert!(
        r.corner_px < MAX_CORNER_SCALE_ERROR_PX,
        "radial scale error {:.6} = {:.3} px at the corner. \
         fov/output-size handling regressed.",
        r.scale,
        r.corner_px
    );
}

/// A cheaper filter may move *sharpness* but never *geometry*. PSNR is printed, not asserted: evidence for the default, not a contract.
#[test]
fn interpolation_changes_sharpness_not_geometry() {
    let (params, src, reference) = fixture();

    let mut rendered: Vec<(&str, Vec<u8>, f64)> = Vec::new();
    println!("filter     PSNR vs app   median disp   corner scale err   sharpness");
    for interp in [Interp::Lanczos4, Interp::Bicubic, Interp::Bilinear] {
        let out = undistort_rgb_image(&params, interp, src.as_raw()).expect("undistort");
        let ours = rgb_to_gray(&out, 1920, 1080);
        let psnr_db = psnr(&ours, &reference);
        let r = register(&ours, &reference);
        let sharp = sharpness(&ours);
        println!(
            "{:<10} {psnr_db:>8.2} dB {:>12.4} px {:>13.3} px {:>11.1}  ({} patches)",
            interp.name(),
            r.median,
            r.corner_px,
            sharp,
            r.patches
        );
        rendered.push((interp.name(), out, sharp));
        assert!(
            r.patches >= MIN_VALID_PATCHES,
            "{}: only {} valid patches",
            interp.name(),
            r.patches
        );
        assert!(
            r.median < MAX_MEDIAN_DISPLACEMENT_PX,
            "{}: median displacement {:.4} px. The filter moved geometry, \
             which no interpolation choice may do.",
            interp.name(),
            r.median
        );
        assert!(
            r.corner_px < MAX_CORNER_SCALE_ERROR_PX,
            "{}: radial scale error {:.3} px at the corner",
            interp.name(),
            r.corner_px
        );
    }

    // Without these the test passes just as happily when every filter resolves to the same kernel.
    for (i, a) in rendered.iter().enumerate() {
        for b in &rendered[i + 1..] {
            assert!(
                a.1 != b.1,
                "{} and {} rendered identical images, so `interp` is not \
                 reaching the kernel.",
                a.0,
                b.0
            );
        }
    }
    // Only bilinear separates here: this metric counts bicubic's ringing as detail, putting it level with lanczos4.
    let (bilinear, others) = rendered.split_last().unwrap();
    assert_eq!(bilinear.0, "bilinear", "filter order changed");
    for other in others {
        assert!(
            bilinear.2 < other.2 * 0.9,
            "bilinear scored {:.1} against {}'s {:.1}: bilinear must be the \
             visibly softest of the three.",
            bilinear.2,
            other.0,
            other.2
        );
    }
}

/// Mean squared Laplacian. Higher is sharper.
fn sharpness(g: &Gray) -> f64 {
    let (w, h) = (g.w, g.h);
    let mut acc = 0.0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let l = g.data[i - 1] + g.data[i + 1] + g.data[i - w] + g.data[i + w] - 4.0 * g.data[i];
            acc += l * l;
        }
    }
    acc / ((w - 2) * (h - 2)) as f64
}

struct Registration {
    median: f64,
    mean: f64,
    max: f64,
    p90: f64,
    scale: f64,
    corner_px: f64,
    patches: usize,
    /// Counted rather than only skipped: a patch more than 5 px out is the regression, not noise.
    spurious: usize,
    /// Rises when a break blanks part of the frame, which would otherwise look like a patch that was never usable.
    low_texture: usize,
}

/// Low-pass, per-patch phase correlation, plus a best-fit radial scale error that a median over mixed patches would dilute.
fn register(ours: &Gray, reference: &Gray) -> Registration {
    let a = gaussian_blur(ours, SIGMA);
    let b = gaussian_blur(reference, SIGMA);
    let (w, h) = (a.w, a.h);
    let mut disp: Vec<(f64, f64, f64, f64)> = Vec::new(); // cx, cy, dx, dy
    let mut spurious = 0usize;
    let mut low_texture = 0usize;
    for gy in 0..GRID_Y {
        for gx in 0..GRID_X {
            let y0 = ((gy as f64 + 0.5) * h as f64 / GRID_Y as f64 - PATCH as f64 / 2.0)
                .clamp(0.0, (h - PATCH) as f64) as usize;
            let x0 = ((gx as f64 + 0.5) * w as f64 / GRID_X as f64 - PATCH as f64 / 2.0)
                .clamp(0.0, (w - PATCH) as f64) as usize;
            let pa = patch(&a, x0, y0, PATCH);
            let pb = patch(&b, x0, y0, PATCH);
            if std_dev(&pa) < 3.0 {
                low_texture += 1;
                continue;
            }
            let (dx, dy) = phase_corr(&pa, &pb, PATCH);
            if dx.abs() > 5.0 || dy.abs() > 5.0 {
                spurious += 1;
                continue;
            }
            disp.push((
                x0 as f64 + PATCH as f64 / 2.0,
                y0 as f64 + PATCH as f64 / 2.0,
                dx,
                dy,
            ));
        }
    }
    if disp.is_empty() {
        return Registration {
            median: f64::INFINITY,
            mean: f64::INFINITY,
            max: f64::INFINITY,
            p90: f64::INFINITY,
            scale: f64::INFINITY,
            corner_px: f64::INFINITY,
            patches: 0,
            spurious,
            low_texture,
        };
    }
    let mut mags: Vec<f64> = disp
        .iter()
        .map(|d| (d.2 * d.2 + d.3 * d.3).sqrt())
        .collect();
    mags.sort_by(f64::total_cmp);

    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (px, py, dx, dy) in &disp {
        let (rx, ry) = (px - cx, py - cy);
        let r = (rx * rx + ry * ry).sqrt().max(1e-9);
        num += r * ((rx * dx + ry * dy) / r);
        den += r * r;
    }
    let scale = num / den;
    Registration {
        median: mags[mags.len() / 2],
        mean: mags.iter().sum::<f64>() / mags.len() as f64,
        max: *mags.last().unwrap(),
        p90: mags[mags.len() * 9 / 10],
        scale,
        corner_px: scale.abs() * (cx * cx + cy * cy).sqrt(),
        patches: disp.len(),
        spurious,
        low_texture,
    }
}
