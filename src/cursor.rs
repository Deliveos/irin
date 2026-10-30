use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::rc::Rc;

use anyhow::{anyhow, ensure, Context};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;
use xcursor::parser::{parse_xcursor, Image};
use xcursor::CursorTheme;

/// Some default looking `left_ptr` icon.
static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../resources/cursor.rgba");

type XCursorCache = HashMap<(CursorIcon, i32), Option<Rc<XCursor>>>;

pub struct CursorManager {
    theme: CursorTheme,
    size: u8,
    current_cursor: CursorImageStatus,
    named_cursor_cache: RefCell<XCursorCache>,
}

impl CursorManager {
    pub fn new(theme: &str, size: u8) -> Self {
        Self::ensure_env(theme, size);

        let theme = CursorTheme::load(theme);

        Self {
            theme,
            size,
            current_cursor: CursorImageStatus::default_named(),
            named_cursor_cache: Default::default(),
        }
    }

    /// Reload the cursor theme.
    pub fn reload(&mut self, theme: &str, size: u8) {
        Self::ensure_env(theme, size);
        self.theme = CursorTheme::load(theme);
        self.size = size;
        self.named_cursor_cache.get_mut().clear();
    }

    /// Checks if the cursor WlSurface is alive, and if not, cleans it up.
    pub fn check_cursor_image_surface_alive(&mut self) {
        if let CursorImageStatus::Surface(surface) = &self.current_cursor {
            if !surface.alive() {
                self.current_cursor = CursorImageStatus::default_named();
            }
        }
    }

    /// Get the current rendering cursor.
    pub fn get_render_cursor(&self, scale: i32) -> RenderCursor {
        match self.current_cursor.clone() {
            CursorImageStatus::Hidden => RenderCursor::Hidden,
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .hotspot
                });

                RenderCursor::Surface { hotspot, surface }
            }
            CursorImageStatus::Named(icon) => self.get_render_cursor_named(icon, scale),
        }
    }

    fn get_render_cursor_named(&self, icon: CursorIcon, scale: i32) -> RenderCursor {
        self.get_cursor_with_name(icon, scale)
            .map(|cursor| RenderCursor::Named {
                icon,
                scale,
                cursor,
            })
            .unwrap_or_else(|| RenderCursor::Named {
                icon: Default::default(),
                scale,
                cursor: self.get_default_cursor(scale),
            })
    }

    /// Returns the area that the current cursor covers at `scale`, relative to the pointer.
    ///
    /// For an animated cursor, this covers every frame. Returns `None` if the cursor is hidden.
    pub fn current_cursor_bbox(&self, scale: i32) -> Option<Rectangle<f64, Logical>> {
        match self.get_render_cursor(scale) {
            RenderCursor::Hidden => None,
            RenderCursor::Surface { hotspot, surface } => {
                Some(bbox_from_surface_tree(&surface, hotspot.upscale(-1)).to_f64())
            }
            RenderCursor::Named { scale, cursor, .. } => cursor
                .frames()
                .iter()
                .map(|frame| {
                    let hotspot = XCursor::hotspot(frame).to_logical(scale).to_f64();
                    let size = Size::from((
                        f64::from(frame.width) / f64::from(scale),
                        f64::from(frame.height) / f64::from(scale),
                    ));
                    Rectangle::new(hotspot.upscale(-1.), size)
                })
                .reduce(|a, b| a.merge(b)),
        }
    }

    pub fn is_current_cursor_animated(&self, scale: i32) -> bool {
        match &self.current_cursor {
            CursorImageStatus::Hidden => false,
            CursorImageStatus::Surface(_) => false,
            CursorImageStatus::Named(icon) => self
                .get_cursor_with_name(*icon, scale)
                .unwrap_or_else(|| self.get_default_cursor(scale))
                .is_animated_cursor(),
        }
    }

    /// Get named cursor for the given `icon` and `scale`.
    pub fn get_cursor_with_name(&self, icon: CursorIcon, scale: i32) -> Option<Rc<XCursor>> {
        self.named_cursor_cache
            .borrow_mut()
            .entry((icon, scale))
            .or_insert_with_key(|(icon, scale)| {
                let size = self.size as i32 * scale;
                let mut cursor = Self::load_xcursor(&self.theme, icon.name(), size);

                // Check alternative names to account for non-compliant themes.
                if cursor.is_err() {
                    for name in icon.alt_names() {
                        cursor = Self::load_xcursor(&self.theme, name, size);
                        if cursor.is_ok() {
                            break;
                        }
                    }
                }

                if let Err(err) = &cursor {
                    warn!("error loading xcursor {}@{size}: {err:?}", icon.name());
                }

                // The default cursor must always have a fallback.
                if *icon == CursorIcon::Default && cursor.is_err() {
                    cursor = Ok(Self::fallback_cursor());
                }

                cursor.ok().map(Rc::new)
            })
            .clone()
    }

    /// Get default cursor.
    pub fn get_default_cursor(&self, scale: i32) -> Rc<XCursor> {
        // The default cursor always has a fallback.
        self.get_cursor_with_name(CursorIcon::Default, scale)
            .unwrap()
    }

    /// Currently used cursor_image as a cursor provider.
    pub fn cursor_image(&self) -> &CursorImageStatus {
        &self.current_cursor
    }

    /// Set new cursor image provider.
    pub fn set_cursor_image(&mut self, cursor: CursorImageStatus) {
        self.current_cursor = cursor;
    }

    /// Load the cursor with the given `name` from the file system picking the closest
    /// one to the given `size`.
    fn load_xcursor(theme: &CursorTheme, name: &str, size: i32) -> anyhow::Result<XCursor> {
        let _span = tracy_client::span!("load_xcursor");

        let path = theme
            .load_icon(name)
            .ok_or_else(|| anyhow!("no default icon"))?;

        let mut file = File::open(path).context("error opening cursor icon file")?;
        let mut buf = vec![];
        file.read_to_end(&mut buf)
            .context("error reading cursor icon file")?;

        check_image_sizes(&buf)?;
        let images = parse_xcursor(&buf).context("error parsing cursor icon file")?;
        XCursor::from_images(images, size)
    }

    /// Set the common XCURSOR env variables.
    fn ensure_env(theme: &str, size: u8) {
        env::set_var("XCURSOR_THEME", theme);
        env::set_var("XCURSOR_SIZE", size.to_string());
    }

    fn fallback_cursor() -> XCursor {
        let images = vec![Image {
            size: 32,
            width: 64,
            height: 64,
            xhot: 1,
            yhot: 1,
            delay: 0,
            pixels_rgba: Vec::from(FALLBACK_CURSOR_DATA),
            pixels_argb: vec![],
        }];

        XCursor {
            images,
            animation_duration: 0,
        }
    }
}

/// The cursor prepared for renderer.
pub enum RenderCursor {
    Hidden,
    Surface {
        hotspot: Point<i32, Logical>,
        surface: WlSurface,
    },
    Named {
        icon: CursorIcon,
        scale: i32,
        cursor: Rc<XCursor>,
    },
}

type TextureCache = HashMap<(CursorIcon, i32), Vec<MemoryRenderBuffer>>;

#[derive(Default)]
pub struct CursorTextureCache {
    cache: RefCell<TextureCache>,
}

impl CursorTextureCache {
    pub fn clear(&mut self) {
        self.cache.get_mut().clear();
    }

    pub fn get(
        &self,
        icon: CursorIcon,
        scale: i32,
        cursor: &XCursor,
        idx: usize,
    ) -> MemoryRenderBuffer {
        self.cache
            .borrow_mut()
            .entry((icon, scale))
            .or_insert_with(|| {
                cursor
                    .frames()
                    .iter()
                    .map(|frame| {
                        MemoryRenderBuffer::from_slice(
                            &frame.pixels_rgba,
                            Fourcc::Argb8888,
                            (frame.width as i32, frame.height as i32),
                            scale,
                            Transform::Normal,
                            None,
                        )
                    })
                    .collect()
            })[idx]
            .clone()
    }
}

/// Checks that the pixels of every image in an Xcursor file fit in the file.
///
/// `parse_xcursor()` allocates the pixels of an image as declared in its header, up to about 4 GiB,
/// before it finds out that the file is shorter, so check this first.
fn check_image_sizes(file: &[u8]) -> anyhow::Result<()> {
    const IMAGE_TYPE: u32 = 0xfffd_0002;
    // Header size, type, nominal size, version, width, height, hotspot x and y, delay.
    const IMAGE_HEADER_LEN: u128 = 36;

    let u32_at = |pos: u64| -> Option<u32> {
        let pos = usize::try_from(pos).ok()?;
        let bytes = file.get(pos..pos.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };

    // Leave malformed headers and tables of contents to parse_xcursor().
    let (Some(header_len), Some(ntoc)) = (u32_at(4), u32_at(12)) else {
        return Ok(());
    };
    for i in 0..u64::from(ntoc) {
        let entry = u64::from(header_len) + i * 12;
        let (Some(toc_type), Some(pos)) = (u32_at(entry), u32_at(entry + 8)) else {
            return Ok(());
        };
        if toc_type != IMAGE_TYPE {
            continue;
        }

        let pos = u64::from(pos);
        let (Some(width), Some(height)) = (u32_at(pos + 16), u32_at(pos + 20)) else {
            return Ok(());
        };
        // In u128, so that this can't overflow for any width and height.
        let len = 4 * u128::from(width) * u128::from(height);
        ensure!(
            u128::from(pos) + IMAGE_HEADER_LEN + len <= file.len() as u128,
            "cursor image of {width} × {height} doesn't fit in the file"
        );
    }

    Ok(())
}

// The XCursorBuffer implementation is inspired by `wayland-rs`, thus provided under MIT license.

/// The state of the `NamedCursor`.
pub struct XCursor {
    /// The image for the underlying named cursor.
    images: Vec<Image>,
    /// The total duration of the animation.
    animation_duration: u32,
}

impl XCursor {
    /// Creates a cursor from the images of an Xcursor file, picking the closest ones to `size`.
    ///
    /// The values come from a file in the cursor theme, so they're not trusted.
    fn from_images(mut images: Vec<Image>, size: i32) -> anyhow::Result<Self> {
        let (width, height) = images
            .iter()
            .min_by_key(|image| (i64::from(size) - i64::from(image.size)).abs())
            .map(|image| (image.width, image.height))
            .context("cursor icon file has no images")?;

        images.retain(move |image| image.width == width && image.height == height);

        // We only use pixels_rgba, so free the other copy of the pixels.
        for image in &mut images {
            image.pixels_argb = Vec::new();
        }

        let animation_duration = images
            .iter()
            .fold(0u32, |acc, image| acc.saturating_add(image.delay));

        Ok(XCursor {
            images,
            animation_duration,
        })
    }

    /// Given a time, calculate which frame to show, and how much time remains until the next frame.
    ///
    /// Time will wrap, so if for instance the cursor has an animation lasting 100ms,
    /// then calling this function with 5ms and 105ms as input gives the same output.
    pub fn frame(&self, mut millis: u32) -> (usize, &Image) {
        if self.animation_duration == 0 {
            return (0, &self.images[0]);
        }

        millis %= self.animation_duration;

        let mut res = 0;
        for (i, img) in self.images.iter().enumerate() {
            if millis < img.delay {
                res = i;
                break;
            }
            millis -= img.delay;
        }

        (res, &self.images[res])
    }

    /// Get the frames for the given `XCursor`.
    pub fn frames(&self) -> &[Image] {
        &self.images
    }

    /// Check whether the cursor is animated.
    pub fn is_animated_cursor(&self) -> bool {
        self.images.len() > 1
    }

    /// Get hotspot for the given `image`.
    pub fn hotspot(image: &Image) -> Point<i32, Physical> {
        (image.xhot as i32, image.yhot as i32).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: u32, delay: u32) -> Image {
        Image {
            size,
            width: 1,
            height: 1,
            xhot: 0,
            yhot: 0,
            delay,
            pixels_rgba: vec![1, 2, 3, 4],
            pixels_argb: vec![4, 1, 2, 3],
        }
    }

    #[test]
    fn file_without_images_is_an_error() {
        // A valid header with no table of contents entries.
        let mut file = Vec::new();
        file.extend_from_slice(b"Xcur");
        file.extend_from_slice(&16u32.to_le_bytes()); // Header size.
        file.extend_from_slice(&0x1_0000u32.to_le_bytes()); // Version.
        file.extend_from_slice(&0u32.to_le_bytes()); // Number of entries.

        let images = parse_xcursor(&file).unwrap();
        assert!(images.is_empty());
        assert!(XCursor::from_images(images, 24).is_err());
    }

    /// An Xcursor file with one image of the given size and the given pixel bytes.
    fn xcursor_file(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut file = Vec::new();
        for value in [16, 0x1_0000, 1] {
            // Header size, version, number of entries.
            file.extend_from_slice(&u32::to_le_bytes(value));
        }
        file.splice(0..0, *b"Xcur");
        // Table of contents: an image of nominal size 24 at position 28.
        for value in [0xfffd_0002, 24, 28] {
            file.extend_from_slice(&u32::to_le_bytes(value));
        }
        // Image header: header size, type, nominal size, version, width, height, hotspot, delay.
        for value in [36, 0xfffd_0002, 24, 1, width, height, 0, 0, 0] {
            file.extend_from_slice(&u32::to_le_bytes(value));
        }
        file.extend_from_slice(pixels);
        file
    }

    #[test]
    fn image_larger_than_the_file_is_rejected_before_parsing() {
        let file = xcursor_file(1, 1, &[1, 2, 3, 4]);
        check_image_sizes(&file).unwrap();
        assert_eq!(parse_xcursor(&file).unwrap().len(), 1);

        // Without the check, parse_xcursor() would allocate 4 × 32767 × 32767 bytes here.
        let file = xcursor_file(32767, 32767, &[1, 2, 3, 4]);
        assert!(check_image_sizes(&file).is_err());

        let file = xcursor_file(2, 1, &[1, 2, 3, 4]);
        assert!(check_image_sizes(&file).is_err());

        let file = xcursor_file(u32::MAX, u32::MAX, &[1, 2, 3, 4]);
        assert!(check_image_sizes(&file).is_err());
    }

    #[test]
    fn image_size_above_i32_max() {
        // 1 << 31 is i32::MIN when cast to i32.
        let images = vec![image(1 << 31, 0), image(u32::MAX, 0)];
        let cursor = XCursor::from_images(images, 24).unwrap();
        assert_eq!(cursor.frames().len(), 2);
    }

    #[test]
    fn huge_animation_delays() {
        let images = vec![image(24, u32::MAX), image(24, u32::MAX), image(24, 5)];
        let cursor = XCursor::from_images(images, 24).unwrap();
        assert_eq!(cursor.animation_duration, u32::MAX);
        assert_eq!(cursor.frame(0).0, 0);
        assert_eq!(cursor.frame(u32::MAX).0, 0);
    }

    #[test]
    fn unused_pixel_copy_is_freed() {
        let cursor = XCursor::from_images(vec![image(24, 0), image(24, 10)], 24).unwrap();
        for frame in cursor.frames() {
            assert_eq!(frame.pixels_rgba, [1, 2, 3, 4]);
            assert!(frame.pixels_argb.is_empty());
            assert_eq!(frame.pixels_argb.capacity(), 0);
        }
    }
}
