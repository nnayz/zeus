//! CoreGraphics rasterization for brand marks.
//!
//! GPUI's GPU path antialiasing is visibly soft at sidebar-row sizes — the
//! Claude starburst's arms melt together at 14 px. AppKit rasterizes the same
//! 24×24 vector data at Retina scale with CoreGraphics quality, cached per
//! (mark, size, color), exactly like the SF Symbols bridge.

use std::collections::HashMap;
use std::ptr;
use std::sync::{Arc, LazyLock, Mutex};

use diri_ui::{BrandMarkKind, PathCommand};
use gpui::{AnyElement, IntoElement, RenderImage, Rgba, img, prelude::*, px};
use image::{Frame, RgbaImage};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBezierPath, NSBitmapFormat, NSBitmapImageRep, NSColor, NSDeviceRGBColorSpace,
    NSGraphicsContext,
};
use objc2_foundation::NSPoint;

const RASTER_SCALE: f32 = 2.0;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct MarkKey {
    kind: BrandMarkKind,
    size_bits: u32,
    inset_bits: u32,
    color: u32,
}

static CACHE: LazyLock<Mutex<HashMap<MarkKey, Option<Arc<RenderImage>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `diri_ui::set_mark_rasterizer` target: crisp raster for solid-color marks.
pub fn raster_mark(kind: BrandMarkKind, size: f32, inset: f32, color: Rgba) -> Option<AnyElement> {
    let image = cached_mark(kind, size, inset, color)?;
    Some(img(image).flex_none().size(px(size)).into_any_element())
}

fn cached_mark(
    kind: BrandMarkKind,
    size: f32,
    inset: f32,
    color: Rgba,
) -> Option<Arc<RenderImage>> {
    let size = size.max(1.0);
    let key = MarkKey {
        kind,
        size_bits: size.to_bits(),
        inset_bits: inset.to_bits(),
        color: color.into(),
    };
    if let Some(cached) = CACHE
        .lock()
        .expect("brand mark cache lock poisoned")
        .get(&key)
        .cloned()
    {
        return cached;
    }
    let image = MainThreadMarker::new().and_then(|_| rasterize(kind, size, inset, color));
    CACHE
        .lock()
        .expect("brand mark cache lock poisoned")
        .insert(key, image.clone());
    image
}

fn rasterize(kind: BrandMarkKind, size: f32, inset: f32, color: Rgba) -> Option<Arc<RenderImage>> {
    let pixels = (size * RASTER_SCALE).ceil().max(2.0) as usize;
    let available = (size * (1.0 - 2.0 * inset).max(0.0)) * RASTER_SCALE;
    let scale = available / 24.0;
    let origin = (pixels as f32 - 24.0 * scale) / 2.0;

    let path = NSBezierPath::bezierPath();
    let map =
        |x: f32, y: f32| NSPoint::new(f64::from(origin + x * scale), f64::from(origin + y * scale));
    let mut current = (0.0_f32, 0.0_f32);
    for command in kind.path_commands() {
        match *command {
            PathCommand::MoveTo(x, y) => {
                path.moveToPoint(map(x, y));
                current = (x, y);
            }
            PathCommand::LineTo(x, y) => {
                path.lineToPoint(map(x, y));
                current = (x, y);
            }
            PathCommand::QuadTo { control, to } => {
                // NSBezierPath is cubic-only; elevate the quadratic.
                let c1 = (
                    current.0 + 2.0 / 3.0 * (control.0 - current.0),
                    current.1 + 2.0 / 3.0 * (control.1 - current.1),
                );
                let c2 = (
                    to.0 + 2.0 / 3.0 * (control.0 - to.0),
                    to.1 + 2.0 / 3.0 * (control.1 - to.1),
                );
                path.curveToPoint_controlPoint1_controlPoint2(
                    map(to.0, to.1),
                    map(c1.0, c1.1),
                    map(c2.0, c2.1),
                );
                current = to;
            }
            PathCommand::CubicTo {
                control_a,
                control_b,
                to,
            } => {
                path.curveToPoint_controlPoint1_controlPoint2(
                    map(to.0, to.1),
                    map(control_a.0, control_a.1),
                    map(control_b.0, control_b.1),
                );
                current = to;
            }
            PathCommand::Close => path.closePath(),
        }
    }

    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            pixels as isize,
            pixels as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            // Premultiplied (default) alpha: AppKit refuses to create a
            // drawing context onto a non-premultiplied rep. Only the alpha
            // channel is read below, which premultiplication leaves intact.
            NSBitmapFormat::empty(),
            0,
            32,
        )?
    };
    let bytes_per_row = bitmap.bytesPerRow() as usize;
    let byte_count = bytes_per_row.checked_mul(pixels)?;
    let data = bitmap.bitmapData();
    if data.is_null() {
        return None;
    }
    // SAFETY: NSBitmapImageRep owns at least bytesPerRow * pixelsHigh bytes
    // for the lifetime of `bitmap`, and the representation is not planar.
    let bitmap_bytes = unsafe { std::slice::from_raw_parts_mut(data, byte_count) };
    bitmap_bytes.fill(0);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    NSColor::whiteColor().setFill();
    path.fill();
    NSGraphicsContext::restoreGraphicsState_class();

    // Tint from coverage alpha, matching the SF Symbols bridge: output is
    // unpremultiplied BGRA for GPUI. The row flip cancels AppKit's bottom-up
    // origin against SVG's top-down coordinates.
    let red = channel(color.r);
    let green = channel(color.g);
    let blue = channel(color.b);
    let color_alpha = color.a.clamp(0.0, 1.0);
    let mut bgra = vec![0_u8; pixels * pixels * 4];
    for output_y in 0..pixels {
        let input_row = &bitmap_bytes[output_y * bytes_per_row..][..pixels * 4];
        let output_row = &mut bgra[output_y * pixels * 4..][..pixels * 4];
        for (source, destination) in input_row
            .chunks_exact(4)
            .zip(output_row.chunks_exact_mut(4))
        {
            let alpha = ((f32::from(source[3]) * color_alpha).round()) as u8;
            destination.copy_from_slice(&[blue, green, red, alpha]);
        }
    }

    let buffer = RgbaImage::from_raw(pixels as u32, pixels as u32, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![Frame::new(
        buffer
    )])))
}

fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
