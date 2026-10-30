use std::cell::RefCell;
use std::sync::Once;

use glam::{Mat3, Vec2};
use niri_config::CornerRadius;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    ffi, Capability, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Frame as _, FrameContext, Offscreen, Texture as _};
use smithay::gpu_span_location;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::background_effect::RenderParams;
use crate::render_helpers::blur::{Blur, BlurOptions};
use crate::render_helpers::renderer::AsGlesFrame as _;
use crate::render_helpers::shaders::{mat3_uniform, Shaders};
use crate::utils::region::TransformedRegion;

#[derive(Debug)]
pub struct FramebufferEffect {
    id: Id,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct FramebufferEffectElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<f64, Logical>,
    clip_geo: Rectangle<f64, Logical>,
    corner_radius: CornerRadius,
    subregion: Option<TransformedRegion>,
    scale: f32,
    blur_options: Option<BlurOptions>,
    noise: f32,
    saturation: f32,
}

#[derive(Debug)]
struct Inner {
    framebuffer: Option<GlesTexture>,
    blur: Option<Blur>,
    intermediate: Option<GlesTexture>,
    /// Reusable storage for subregion-filtered damage rects.
    subregion_damage: Vec<Rectangle<i32, Physical>>,
}

impl FramebufferEffect {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
        }
    }

    pub fn damage(&mut self) {
        self.commit.increment();
    }

    pub fn render(
        &self,
        ns: Option<usize>,
        params: RenderParams,
        blur_options: Option<BlurOptions>,
        noise: f32,
        saturation: f32,
    ) -> FramebufferEffectElement {
        let (clip_geo, corner_radius) = params
            .clip
            .unwrap_or((params.geometry, CornerRadius::default()));

        let mut id = self.id.clone();
        if let Some(ns) = ns {
            id = id.namespaced(ns);
        }

        FramebufferEffectElement {
            id,
            commit: self.commit,
            geometry: params.geometry,
            clip_geo,
            corner_radius,
            subregion: params.subregion,
            scale: params.scale as f32,
            blur_options,
            noise,
            saturation,
        }
    }
}

impl FramebufferEffectElement {
    fn compute_uniforms(
        &self,
        crop: Rectangle<f64, Logical>,
        transform: Transform,
    ) -> [Uniform<'static>; 7] {
        let offset = crop.loc - (self.clip_geo.loc - self.geometry.loc);
        let offset = Vec2::new(offset.x as f32, offset.y as f32);
        let crop_size = Vec2::new(crop.size.w as f32, crop.size.h as f32);
        let clip_size = Vec2::new(self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        // Our v_coords are [0, 1] inside crop. We want them to be [0, 1] inside clip_geo.
        let input_to_clip_geo =
            Mat3::from_scale(crop_size / clip_size) * Mat3::from_translation(offset / crop_size);

        // Revert the effect of the texture transform.
        let transform_mat = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * transform.matrix()
            * Mat3::from_translation(Vec2::new(-0.5, -0.5));
        let input_to_clip_geo = input_to_clip_geo * transform_mat;

        let clip_geo_size = (self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        [
            Uniform::new("niri_scale", self.scale),
            Uniform::new("geo_size", clip_geo_size),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            mat3_uniform("input_to_geo", input_to_clip_geo),
            Uniform::new("noise", self.noise),
            Uniform::new("saturation", self.saturation),
            Uniform::new("bg_color", [0f32, 0., 0., 0.]),
        ]
    }

    /// Returns the part of `dst` that the captured texture covers.
    ///
    /// `clamped_dst` is the part of `dst` inside the output. On each axis where a texture for the
    /// whole element fits in the output size, the texture covers the whole `dst`, so that its size
    /// doesn't change while the element moves across the output edge, and is at most the output
    /// size. On the other axes, it covers only `clamped_dst`, and its size is that of the on-screen
    /// part divided by the zoom (see capture_framebuffer()), which can exceed the output size for a
    /// zoomed-out element.
    fn texture_dst(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        clamped_dst: Rectangle<i32, Physical>,
        output_size: Size<i32, Physical>,
    ) -> Rectangle<i32, Physical> {
        let full_size: Size<i32, Physical> = src
            .size
            .to_logical(1., Transform::Normal)
            .to_physical_precise_round(self.scale);
        let mut texture_dst = clamped_dst;
        if full_size.w <= output_size.w {
            texture_dst.loc.x = dst.loc.x;
            texture_dst.size.w = dst.size.w;
        }
        if full_size.h <= output_size.h {
            texture_dst.loc.y = dst.loc.y;
            texture_dst.size.h = dst.size.h;
        }
        texture_dst
    }
}

impl Element for FramebufferEffectElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        // We don't use src for drawing but we can use it to figure out how we were cropped.
        let size = self.geometry.size.to_buffer(1., Transform::Normal);
        Rectangle::from_size(size)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry.to_physical_precise_round(scale)
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlesRenderer> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        let _span = tracy_client::span!("FramebufferEffectElement::capture_framebuffer");
        let location = gpu_span_location!("FramebufferEffectElement::capture_framebuffer");
        frame.with_gpu_span(location, |frame| {
            let output_rect = Rectangle::from_size(frame.output_size());
            let transform = frame.transformation();

            let supports_blit = frame.capabilities().contains(&Capability::Blit);
            let mut guard = frame.renderer();

            let inner = cache
                .get_or_insert::<RefCell<Inner>, _>(|| RefCell::new(Inner::new(guard.as_mut())));
            let mut inner = inner.borrow_mut();
            let inner = &mut *inner;

            inner.intermediate = None;

            if !supports_blit {
                static WARN: Once = Once::new();
                WARN.call_once(|| {
                    warn!("non-xray background effects disabled: renderer does not support framebuffer blitting");
                });
                return Ok(());
            }

            // We want clamp-to-edge behavior for out-of-bounds pixels. However, glBlitFramebuffer
            // seems to skip out-of-bounds pixels, even though my reading of the docs suggests
            // otherwise (we use GL_LINEAR filter). So, blit only the part of dst inside the
            // framebuffer bounds, and stretch its edge pixels over the rest of the texture.
            let clamped_dst = match dst.intersection(output_rect) {
                Some(clamped) => clamped,
                None => return Ok(()),
            };
            let texture_dst = self.texture_dst(src, dst, clamped_dst, output_rect.size);
            let texture_scale = texture_dst.size.to_f64() / dst.size.to_f64();

            let visible = transform.transform_rect_in(clamped_dst, &output_rect.size);
            let texture_dst = transform.transform_rect_in(texture_dst, &output_rect.size);

            // Compute size from our geometry and scale.
            //
            // The "correct" size is always texture_dst.size since that's the part of the
            // framebuffer that the texture covers. However, using texture_dst.size causes two
            // undesirable things when zooming out for the overview:
            // 1. texture_dst.size shrinks every frame, causing a texture realloaction for every fb
            //    effect element every frame.
            // 2. The underlying blur visually expands. This is technically correct, since the
            //    underlying contents shrink, but it's not what you visually expect: you expect the
            //    blur to also shrink as the windows zoom out, to give the zooming out effect.
            //
            // Using size computed from geometry and scale solves both of those problems (even
            // though there's a bit of a cost in that zoomed-out elements still blur the entire
            // unzoomed texture size, and even though the blur ends up slightly wrong as there's two
            // layers of texture resampling, up and back down).
            //
            // Here we use src.size rather than geometry directly because src takes into account
            // cropping.
            let size = src
                .size
                .to_logical(1., Transform::Normal)
                .upscale(texture_scale)
                .to_physical_precise_round(self.scale);
            let size = transform.transform_size(size);

            let size = size.to_logical(1).to_buffer(1, Transform::Normal);

            // Where the visible part goes in the texture.
            let to_texture = |x: i32, start: i32, len: i32, texture_len: i32| {
                let x = f64::from(x - start) * f64::from(texture_len) / f64::from(len);
                (x.round() as i32).clamp(0, texture_len)
            };
            let (vx0, vx1) = (visible.loc.x, visible.loc.x + visible.size.w);
            let (vy0, vy1) = (visible.loc.y, visible.loc.y + visible.size.h);
            let (x0, y0) = (texture_dst.loc.x, texture_dst.loc.y);
            let (w, h) = (texture_dst.size.w, texture_dst.size.h);
            let (tx0, tx1) = (to_texture(vx0, x0, w, size.w), to_texture(vx1, x0, w, size.w));
            let (ty0, ty1) = (to_texture(vy0, y0, h, size.h), to_texture(vy1, y0, h, size.h));

            // A 3 × 3 grid of blits as (source start, source end, texture start, texture end) on
            // each axis: the visible part in the middle, and its edge rows and columns stretched
            // to the texture edges. On an axis where the texture covers only the visible part, the
            // edge entries are empty.
            //
            // The edge and corner blits use GL_NEAREST, which repeats the one-pixel source row or
            // column exactly. With GL_LINEAR, the driver may sample the neighboring pixels outside
            // the source region, and the stretched rows and columns would fade into them.
            let columns = [(vx0, vx0 + 1, 0, tx0), (vx0, vx1, tx0, tx1), (vx1 - 1, vx1, tx1, size.w)];
            let rows = [(vy0, vy0 + 1, 0, ty0), (vy0, vy1, ty0, ty1), (vy1 - 1, vy1, ty1, size.h)];

            // Recreate framebuffer if needed.
            if inner
                .framebuffer
                .as_ref()
                .is_some_and(|fb| fb.size() != size)
            {
                inner.framebuffer = None;
            }
            let framebuffer = if let Some(fb) = &inner.framebuffer {
                fb
            } else {
                trace!("creating framebuffer texture sized {} × {}", size.w, size.h);
                let renderer = guard.as_mut();
                let texture = renderer.create_buffer(Fourcc::Abgr8888, size)?;
                inner.framebuffer.insert(texture)
            };

            // Prepare blur textures.
            let mut blur = Option::zip(inner.blur.as_mut(), self.blur_options);
            if let Some((b, options)) = &mut blur {
                let renderer = guard.as_mut();
                if let Err(err) = b.prepare_textures(
                    |fourcc, size| renderer.create_buffer(fourcc, size),
                    framebuffer,
                    *options,
                ) {
                    warn!("error preparing blur textures: {err:?}");
                    blur = None;
                }
            }

            // We can't use renderer.with_context() as that will reset the GlesFrame binding that we
            // want to blit from.
            drop(guard);

            // Blit the framebuffer contents.
            frame.with_context(|gl| unsafe {
                while gl.GetError() != ffi::NO_ERROR {}

                let mut current_fbo = 0i32;
                gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo as *mut _);

                // BlitFramebuffer is affected by the scissor test, we don't want that.
                gl.Disable(ffi::SCISSOR_TEST);

                let mut fbo = 0;
                gl.GenFramebuffers(1, &mut fbo as *mut _);
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);

                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    framebuffer.tex_id(),
                    0,
                );

                for (i, (src_x0, src_x1, dst_x0, dst_x1)) in columns.into_iter().enumerate() {
                    for (j, (src_y0, src_y1, dst_y0, dst_y1)) in rows.into_iter().enumerate() {
                        if dst_x0 >= dst_x1 || dst_y0 >= dst_y1 {
                            continue;
                        }

                        let filter = if (i, j) == (1, 1) {
                            ffi::LINEAR
                        } else {
                            ffi::NEAREST
                        };
                        gl.BlitFramebuffer(
                            src_x0,
                            src_y0,
                            src_x1,
                            src_y1,
                            dst_x0,
                            dst_y0,
                            dst_x1,
                            dst_y1,
                            ffi::COLOR_BUFFER_BIT,
                            filter,
                        );
                    }
                }

                // Restore state set by GlesFrame that we just modified.
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, current_fbo as u32);
                gl.Enable(ffi::SCISSOR_TEST);

                gl.DeleteFramebuffers(1, &mut fbo as *mut _);

                if gl.GetError() != ffi::NO_ERROR {
                    Err(GlesError::BlitError)
                } else {
                    Ok(())
                }
            })??;

            // If blur is off, use the unblurred texture.
            if self.blur_options.is_none() {
                inner.intermediate = Some(framebuffer.clone());
                return Ok(());
            }

            if let Some((blur, options)) = blur {
                let mut guard = frame.renderer();
                let renderer = guard.as_mut();
                match blur.render(renderer, framebuffer, options) {
                    Ok(blurred) => inner.intermediate = Some(blurred),
                    Err(err) => {
                        warn!("error rendering blur: {err:?}");
                    }
                }
            }

            Ok(())
        })
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let Some(cache) = cache else {
            return Ok(());
        };
        let Some(inner) = cache.get::<RefCell<Inner>>() else {
            return Ok(());
        };
        let mut inner = inner.borrow_mut();
        let inner = &mut *inner;

        let Some(texture) = &inner.intermediate else {
            return Ok(());
        };

        // Draw the texture where capture_framebuffer() captured it.
        let output_rect = Rectangle::from_size(frame.output_size());
        let clamped_dst = match dst.intersection(output_rect) {
            Some(clamped) => clamped,
            None => return Ok(()),
        };
        let texture_dst = self.texture_dst(src, dst, clamped_dst, output_rect.size);
        let clamp_offset = texture_dst.loc - dst.loc;

        // Filter damage by subregion, reusing the stored Vec to avoid allocation.
        let filtered = &mut inner.subregion_damage;
        filtered.clear();

        if let Some(subregion) = &self.subregion {
            // Convert to subregion coordinates.
            let mut crop = src.to_logical(1., Transform::Normal, &src.size);
            crop.loc += self.geometry.loc;
            subregion.filter_damage(crop, dst, damage, filtered);
        } else {
            filtered.extend(damage.iter());
        };

        // Adjust for the texture covering only part of dst.
        if texture_dst != dst {
            let r = Rectangle::new(clamp_offset, texture_dst.size);
            filtered.retain_mut(|d| {
                if let Some(mut crop) = d.intersection(r) {
                    crop.loc -= clamp_offset;
                    *d = crop;
                    true
                } else {
                    false
                }
            });
        }

        if filtered.is_empty() {
            return Ok(());
        }
        let damage = &filtered[..];

        // Adjust src proportionally to the part of dst that the texture covers.
        let src_loc = src.loc.to_logical(1., Transform::Normal, &src.size);
        let dst_to_src = src.size / dst.size.to_f64();
        let crop = Rectangle::new(
            src_loc + clamp_offset.to_f64().upscale(dst_to_src).to_logical(1.),
            texture_dst.size.to_f64().upscale(dst_to_src).to_logical(1.),
        );

        let program = Shaders::get_from_frame(frame).postprocess_and_clip.clone();
        let uniforms = program
            .is_some()
            .then(|| self.compute_uniforms(crop, frame.transformation()));
        let uniforms = uniforms.as_ref().map_or(&[][..], |x| &x[..]);

        frame.render_texture_from_to(
            texture,
            Rectangle::from_size(texture.size().to_f64()),
            texture_dst,
            damage,
            &[],
            // The intermediate texture has the same transform as the frame.
            frame.transformation().invert(),
            1.,
            program.as_ref(),
            uniforms,
        )
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::capture_framebuffer(&self, gles_frame, src, dst, cache)?;
        Ok(())
    }

    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::draw(
            &self,
            gles_frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }
}

impl Inner {
    fn new(renderer: &mut GlesRenderer) -> Self {
        Inner {
            framebuffer: None,
            blur: Blur::new(renderer),
            intermediate: None,
            subregion_damage: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::egl::native::EGLSurfacelessDisplay;
    use smithay::backend::egl::{EGLContext, EGLDisplay};
    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _, Renderer as _};

    use super::*;
    use crate::niri_render_elements;
    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

    niri_render_elements! {
        TestElement => {
            Solid = SolidColorRenderElement,
            Effect = FramebufferEffectElement,
        }
    }

    fn renderer() -> GlesRenderer {
        let mut renderer = unsafe {
            let display = EGLDisplay::new(EGLSurfacelessDisplay).unwrap();
            let context = EGLContext::new(&display).unwrap();
            GlesRenderer::new(context).unwrap()
        };
        crate::render_helpers::shaders::init(&mut renderer);
        renderer
    }

    fn effect(
        geometry: (f64, f64, f64, f64),
        blur_options: Option<BlurOptions>,
        saturation: f32,
    ) -> FramebufferEffectElement {
        let (x, y, w, h) = geometry;
        let params = RenderParams {
            geometry: Rectangle::new((x, y).into(), (w, h).into()),
            subregion: None,
            clip: None,
            scale: 1.,
        };
        FramebufferEffect::new().render(None, params, blur_options, 0., saturation)
    }

    /// Captures the framebuffer for the effect into `cache` on a 16 × 16 output where every pixel
    /// has a different color, and returns the output texture.
    fn capture(
        renderer: &mut GlesRenderer,
        elem: &FramebufferEffectElement,
        cache: &UserDataMap,
    ) -> GlesTexture {
        let size = Size::from((16, 16));
        let mut texture: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, Size::from((16, 16)))
            .unwrap();
        let mut target = renderer.bind(&mut texture).unwrap();
        let mut frame = renderer
            .render(&mut target, size, Transform::Normal)
            .unwrap();
        for y in 0..16_i32 {
            for x in 0..16_i32 {
                let color = Color32F::new(
                    (x * 16 + 8) as f32 / 255.,
                    (y * 16 + 8) as f32 / 255.,
                    0.,
                    1.,
                );
                let pixel: Rectangle<i32, Physical> = Rectangle::new((x, y).into(), (1, 1).into());
                frame.clear(color, &[pixel]).unwrap();
            }
        }
        let dst = elem.geometry(Scale::from(1.));
        RenderElement::<GlesRenderer>::capture_framebuffer(
            elem,
            &mut frame,
            elem.src(),
            dst,
            cache,
        )
        .unwrap();
        let sync = frame.finish().unwrap();
        renderer.wait(&sync).unwrap();
        drop(target);
        texture
    }

    /// Reads back the pixels of the texture.
    fn read(renderer: &mut GlesRenderer, texture: &GlesTexture) -> Vec<u8> {
        let region = Rectangle::from_size(texture.size());
        let mapping = renderer
            .copy_texture(texture, region, Fourcc::Abgr8888)
            .unwrap();
        renderer.map_texture(&mapping).unwrap().to_vec()
    }

    #[test]
    fn egl_texture_size_stays_while_crossing_the_output_edge() {
        let mut renderer = renderer();
        let cache = UserDataMap::new();
        let blur = Some(BlurOptions {
            passes: 2,
            offset: 1.,
        });

        let mut ids = Vec::new();
        for x in [-6., -4., -2., 0., 4., 10.] {
            capture(&mut renderer, &effect((x, 4., 8., 8.), blur, 1.), &cache);
            let inner = cache.get::<RefCell<Inner>>().unwrap().borrow();
            let framebuffer = inner.framebuffer.as_ref().unwrap();
            assert_eq!(framebuffer.size(), Size::from((8, 8)), "x = {x}");
            assert!(inner.intermediate.is_some());
            ids.push(framebuffer.tex_id());
        }
        assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");

        // An element taller than the output gets a texture for its whole width and its on-screen
        // height, so the size also stays while it crosses the side edges.
        ids.clear();
        for x in [-6., -2., 0., 10.] {
            capture(&mut renderer, &effect((x, -2., 8., 20.), blur, 1.), &cache);
            let inner = cache.get::<RefCell<Inner>>().unwrap().borrow();
            let framebuffer = inner.framebuffer.as_ref().unwrap();
            assert_eq!(framebuffer.size(), Size::from((8, 16)), "x = {x}");
            ids.push(framebuffer.tex_id());
        }
        assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");

        // An element wider than the output gets a texture for its on-screen width only.
        capture(&mut renderer, &effect((-6., 4., 20., 8.), blur, 1.), &cache);
        let inner = cache.get::<RefCell<Inner>>().unwrap().borrow();
        let framebuffer = inner.framebuffer.as_ref().unwrap();
        assert_eq!(framebuffer.size(), Size::from((14, 8)));
    }

    #[test]
    fn egl_off_screen_part_repeats_the_on_screen_edge_pixels() {
        let mut renderer = renderer();

        // Geometry on the 16 × 16 output, the output position of the texture's (0, 0), and the
        // texture size.
        let cases = [
            ((-4., -4., 8., 8.), (-4, -4), (8, 8)),
            ((12., -4., 8., 8.), (12, -4), (8, 8)),
            ((-4., 12., 8., 8.), (-4, 12), (8, 8)),
            ((12., 12., 8., 8.), (12, 12), (8, 8)),
            // Taller than the output, so the texture covers only the on-screen height.
            ((-4., -2., 8., 20.), (-4, 0), (8, 16)),
        ];

        for (geometry, origin, size) in cases {
            let cache = UserDataMap::new();
            let output = capture(&mut renderer, &effect(geometry, None, 1.), &cache);
            let output = read(&mut renderer, &output);

            let inner = cache.get::<RefCell<Inner>>().unwrap();
            let framebuffer = inner.borrow().framebuffer.clone().unwrap();
            assert_eq!(framebuffer.size(), Size::from(size), "{geometry:?}");
            let texture = read(&mut renderer, &framebuffer);

            // Every texture pixel has the color of the nearest output pixel.
            for ty in 0..size.1 {
                for tx in 0..size.0 {
                    let ox = (origin.0 + tx).clamp(0, 15);
                    let oy = (origin.1 + ty).clamp(0, 15);
                    let t = ((ty * size.0 + tx) * 4) as usize;
                    let o = ((oy * 16 + ox) * 4) as usize;
                    assert_eq!(
                        texture[t..t + 4],
                        output[o..o + 4],
                        "{geometry:?} at ({tx}, {ty})"
                    );
                }
            }
        }
    }

    #[test]
    fn egl_partly_off_screen_effect_draws_like_its_on_screen_part() {
        let mut renderer = renderer();

        // Every background pixel has a different brightness, so that the grayscale effect shows
        // where each pixel came from.
        let pixels: Vec<_> = (0..64)
            .map(|i| SolidColorBuffer::new((1., 1.), [0., i as f32 * 4. / 255., 0., 1.]))
            .collect();

        let mut render = |transform: Transform, geometry: Option<(f64, f64, f64, f64)>| {
            let mut elements: Vec<TestElement> = pixels
                .iter()
                .enumerate()
                .map(|(i, buffer)| {
                    let loc = ((i % 8) as f64, (i / 8) as f64);
                    SolidColorRenderElement::from_buffer(buffer, loc, 1., Kind::Unspecified).into()
                })
                .collect();
            // Elements are drawn in order, so the effect goes on top.
            elements.extend(geometry.map(|geo| effect(geo, None, 0.).into()));

            let size = Size::from((8, 8));
            let elements = elements.into_iter();
            render_to_vec(
                &mut renderer,
                size,
                Scale::from(1.),
                transform,
                Fourcc::Abgr8888,
                elements,
            )
            .unwrap()
        };

        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let plain = render(transform, None);

            // Partly off-screen geometry, and its on-screen part.
            for (geometry, on_screen) in [
                ((-4., 2., 8., 4.), (0., 2., 4., 4.)),
                ((4., 2., 8., 4.), (4., 2., 4., 4.)),
                ((2., -4., 4., 8.), (2., 0., 4., 4.)),
                ((2., 4., 4., 8.), (2., 4., 4., 4.)),
                ((-2., -3., 6., 6.), (0., 0., 4., 3.)),
                // Wider than the output, so the texture covers only the on-screen part.
                ((-4., 2., 16., 4.), (0., 2., 8., 4.)),
                // Taller than the output, so the texture covers the whole width but only the
                // on-screen height.
                ((-4., -2., 8., 12.), (0., 0., 4., 8.)),
            ] {
                let expected = render(transform, Some(on_screen));
                assert_ne!(expected, plain, "{transform:?} {on_screen:?}");

                let actual = render(transform, Some(geometry));
                assert_eq!(actual, expected, "{transform:?} {geometry:?}");
            }
        }
    }
}
