//! Shared raw raster images. Workers convert pixels once; Blitz paints them directly without
//! PNG encoding, URL caches, asynchronous resource callbacks, or copies during gestures.
use crate::preview::{DisplayImage, PreviewUpdate};
use anyrender::PaintScene;
use dioxus::prelude::*;
use peniko::{Blob, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ImageSource(pub Arc<ImageData>);
impl PartialEq for ImageSource {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl ImageSource {
    pub fn rgb(image: &DisplayImage) -> anyhow::Result<Self> {
        anyhow::ensure!(
            image.data.len() == image.size.px() * 3,
            "invalid RGB preview dimensions"
        );
        let mut rgba = Vec::with_capacity(image.size.px() * 4);
        for pixel in image.data.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        Ok(Self::rgba_pixels(rgba, image.size.w, image.size.h))
    }
    fn rgba(image: &DisplayImage) -> anyhow::Result<Self> {
        anyhow::ensure!(
            image.data.len() == image.size.px() * 4,
            "invalid RGBA overlay dimensions"
        );
        Ok(Self::rgba_pixels(
            image.data.clone(),
            image.size.w,
            image.size.h,
        ))
    }
    fn rgba_pixels(pixels: Vec<u8>, width: u32, height: u32) -> Self {
        Self(Arc::new(ImageData {
            data: Blob::from(pixels),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }))
    }
}

pub fn for_update(update: &PreviewUpdate) -> anyhow::Result<Vec<ImageSource>> {
    Ok(match update {
        PreviewUpdate::Raw(data) => {
            let source = ImageSource::rgb(&data.source)?;
            let output =
                if data.source.size == data.output.size && data.source.data == data.output.data {
                    source.clone()
                } else {
                    ImageSource::rgb(&data.output)?
                };
            vec![source, output]
        }
        PreviewUpdate::Rendered(image) => vec![ImageSource::rgb(image)?],
        PreviewUpdate::Finished {
            overlay: Some(image),
            ..
        } => vec![ImageSource::rgba(image)?],
        _ => Vec::new(),
    })
}

/// Props compare shared image identities, so changing surrounding layout never copies pixels.
#[component]
pub fn RasterImage(
    source: ImageSource,
    #[props(default)] class: String,
    #[props(default)] style: String,
    alt: String,
    #[props(default = false)] nearest: bool,
    #[props(default = false)] cover: bool,
    #[props(default = false)] align_left: bool,
    #[props(default)] view: Option<[f32; 3]>,
) -> Element {
    let mut brush = ImageBrush::new((*source.0).clone());
    brush.sampler.quality = if nearest {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    };
    let widget = dioxus_native::CustomWidgetAttr::new(RasterWidget {
        brush,
        cover,
        align_left,
        view,
    });
    rsx! { object { class, style, role: "img", aria_label: alt, "data": widget } }
}

struct RasterWidget {
    brush: ImageBrush,
    cover: bool,
    align_left: bool,
    view: Option<[f32; 3]>,
}
impl dioxus_native::Widget for RasterWidget {
    fn paint(
        &mut self,
        _context: &mut dyn anyrender::RenderContext,
        _styles: &blitz_dom::node::ComputedStyles,
        width: u32,
        height: u32,
        _scale: f64,
    ) -> anyrender::Scene {
        let mut scene = anyrender::Scene::new();
        if self.brush.image.width != 0 && self.brush.image.height != 0 {
            use peniko::kurbo::{Affine, Rect};
            let mut transform = fit_transform(
                self.brush.image.width,
                self.brush.image.height,
                width,
                height,
                self.cover,
            );
            if self.align_left {
                let mut coefficients = transform.as_coeffs();
                coefficients[4] = 0.;
                transform = Affine::new(coefficients);
            }
            if let Some(view) = self.view {
                transform = viewport_transform(
                    self.brush.image.width,
                    self.brush.image.height,
                    width,
                    height,
                    view,
                );
            }
            scene.push_clip_layer(
                peniko::Fill::NonZero,
                Affine::IDENTITY,
                &Rect::new(0., 0., width as f64, height as f64),
            );
            scene.draw_image(self.brush.as_ref(), transform);
            scene.pop_layer();
        }
        scene
    }
}

/// The original UV viewport: center=.5+pan, visible UV size=1/zoom. Source/output/mask
/// share this transform, and the element itself has the output image's fitted aspect ratio.
fn viewport_transform(
    image_w: u32,
    image_h: u32,
    width: u32,
    height: u32,
    view: [f32; 3],
) -> peniko::kurbo::Affine {
    let [zoom, pan_x, pan_y] = view.map(f64::from);
    peniko::kurbo::Affine::translate((
        (0.5 - zoom * (0.5 + pan_x)) * f64::from(width),
        (0.5 - zoom * (0.5 + pan_y)) * f64::from(height),
    )) * peniko::kurbo::Affine::scale_non_uniform(
        f64::from(width) * zoom / f64::from(image_w),
        f64::from(height) * zoom / f64::from(image_h),
    )
}

fn fit_transform(
    image_w: u32,
    image_h: u32,
    width: u32,
    height: u32,
    cover: bool,
) -> peniko::kurbo::Affine {
    let sx = width as f64 / image_w as f64;
    let sy = height as f64 / image_h as f64;
    let scale = if cover { sx.max(sy) } else { sx.min(sy) };
    peniko::kurbo::Affine::translate((
        (width as f64 - image_w as f64 * scale) * 0.5,
        (height as f64 - image_h as f64 * scale) * 0.5,
    )) * peniko::kurbo::Affine::scale(scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reconst_prep_core::frame::Size;
    #[test]
    fn preview_zoom_matches_original_uv_viewport() {
        use peniko::kurbo::Point;
        let center = Point::new(200., 150.);
        assert_eq!(
            viewport_transform(400, 300, 800, 600, [1., 0., 0.]) * center,
            Point::new(400., 300.)
        );
        assert_eq!(
            viewport_transform(400, 300, 800, 600, [2., 0., 0.]) * center,
            Point::new(400., 300.)
        );
        // At 2x, a UV pan of .25 brings the right edge exactly to the viewport edge.
        let transform = viewport_transform(400, 300, 800, 600, [2., 0.25, 0.]);
        assert_eq!(transform * Point::new(300., 150.), Point::new(400., 300.));
        assert_eq!(transform * Point::new(400., 300.), Point::new(800., 900.));
    }
    #[test]
    fn raster_preserves_pixels_and_shares_storage() {
        let image = DisplayImage {
            size: Size::new(2, 1),
            data: vec![255, 0, 0, 0, 255, 0],
        };
        let source = ImageSource::rgb(&image).unwrap();
        let copy = source.clone();
        assert!(Arc::ptr_eq(&source.0, &copy.0));
        assert_eq!(source.0.data.data(), &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!((source.0.width, source.0.height), (2, 1));
    }
    #[test]
    fn contain_and_cover_preserve_aspect_and_center() {
        assert_eq!(
            fit_transform(200, 100, 100, 100, false).as_coeffs(),
            [0.5, 0., 0., 0.5, 0., 25.]
        );
        assert_eq!(
            fit_transform(200, 100, 100, 100, true).as_coeffs(),
            [1., 0., 0., 1., -50., 0.]
        );
    }
    #[test]
    fn overlay_preserves_straight_alpha() {
        let source = ImageSource::rgba(&DisplayImage {
            size: Size::new(1, 1),
            data: vec![200, 100, 50, 80],
        })
        .unwrap();
        assert_eq!(source.0.alpha_type, ImageAlphaType::Alpha);
        assert_eq!(source.0.data.data(), &[200, 100, 50, 80]);
    }
}
