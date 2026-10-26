use std::fmt::Write as _;
use std::time::Duration;

use insta::assert_snapshot;
use niri_config::animations::{Curve, EasingParams, Kind};
use niri_config::Config;
use niri_ipc::SizeChange;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _};
use smithay::utils::{Point, Size};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::*;
use crate::layout::ActivateWindow;
use crate::niri::{Niri, State};
use crate::render_helpers::{copy_framebuffer, create_texture, RenderCtx, RenderTarget};

fn format_tiles(niri: &Niri) -> String {
    let mut buf = String::new();
    let ws = niri.layout.active_workspace().unwrap();
    let mut tiles: Vec<_> = ws.tiles_with_render_positions().collect();

    // We sort by id since that gives us a consistent order (from first opened to last), but we
    // don't print the id since it's nondeterministic (the id is a global counter across all
    // running tests in the same binary).
    tiles.sort_by_key(|(tile, _, _)| tile.window().id().get());
    for (tile, pos, _visible) in tiles {
        let Size { w, h, .. } = tile.animated_tile_size();
        let Point { x, y, .. } = pos;
        writeln!(&mut buf, "{w:>3.0} × {h:>3.0} at x:{x:>3.0} y:{y:>3.0}").unwrap();
    }
    buf
}

fn create_window(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(w, h);
    window.ack_last_and_commit();
    f.roundtrip(id);

    surface
}

fn set_time(niri: &mut Niri, time: Duration) {
    // This is a bit involved because we're dealing with an AdjustableClock that maintains its own
    // internal current_time.

    // First, reset current_time to zero by matching unadjusted time to it (at rate 0.0), then
    // setting unadjusted time to zero at rate 1.0 (causing current_time to also go to zero).
    let now = niri.clock.now();
    niri.clock.set_unadjusted(now);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(Duration::ZERO);
    niri.clock.set_rate(1.0);
    let _ = niri.clock.now();

    // Now, set the desired time at rate 1.0.
    niri.clock.set_unadjusted(time);
    let _ = niri.clock.now();

    // Freeze the clock so that clear() inside the niri loop callback followed by some get()
    // doesn't replace it with the monotonic time.
    niri.clock.set_rate(0.0);
}

// Sets up a fixture with linear animations, a renderer, and an output.
fn set_up() -> Fixture {
    set_up_with(|_| ())
}

fn set_up_with(adjust_config: impl FnOnce(&mut Config)) -> Fixture {
    const LINEAR: Kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });

    let mut config = Config::default();
    config.layout.gaps = 0.0;
    config.animations.window_resize.anim.kind = LINEAR;
    config.animations.window_movement.0.kind = LINEAR;
    // Keep the overlay out of the rendered pixels.
    config.hotkey_overlay.skip_at_startup = true;
    adjust_config(&mut config);

    let mut f = Fixture::with_config(config);
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (1920, 1080));

    f
}

// Renders an output with a 1920×1080 mode the way a backend does, including the visibility
// tracking, and returns the pixels as RGBA.
fn render_output(f: &mut Fixture, n: u8) -> Vec<u8> {
    let output = f.niri_output(n);
    let State { backend, niri, .. } = f.niri_state();
    niri.update_render_elements(Some(&output));

    backend
        .with_primary_renderer(|renderer| {
            let ctx = RenderCtx {
                renderer: &mut *renderer,
                target: RenderTarget::Output,
                xray: None,
            };
            let elements = niri.render_to_vec(ctx, &output, false);

            let mut damage_tracker = OutputDamageTracker::from_output(&output);
            let size = Size::from((1920, 1080));
            let mut texture = create_texture(renderer, size, Fourcc::Abgr8888).unwrap();
            let mut target = renderer.bind(&mut texture).unwrap();
            let res = damage_tracker
                .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
                .unwrap();
            niri.update_primary_scanout_output(&output, &res.states);

            let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
            renderer.map_texture(&mapping).unwrap().to_vec()
        })
        .unwrap()
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * 1920 + x) * 4;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

fn set_up_two_in_column() -> (Fixture, ClientId, WlSurface, WlSurface) {
    let mut f = set_up();

    let id = f.add_client();

    let surface1 = create_window(&mut f, id, 100, 100);
    let surface2 = create_window(&mut f, id, 200, 200);
    f.double_roundtrip(id);

    let _ = f.client(id).window(&surface1).recent_configures();
    let _ = f.client(id).window(&surface2).recent_configures();

    // Consume into one column.
    f.niri().layout.focus_left();
    f.niri().layout.consume_into_column();
    f.double_roundtrip(id);

    // Commit for the column consume.
    let window = f.client(id).window(&surface1);
    window.ack_last_and_commit();

    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    f.double_roundtrip(id);

    set_time(f.niri(), Duration::ZERO);
    f.niri_complete_animations();

    (f, id, surface1, surface2)
}

#[test]
fn egl_height_resize_animates_next_y() {
    let (mut f, id, surface1, surface2) = set_up_two_in_column();

    // Issue a resize.
    f.niri()
        .layout
        .set_window_height(None, SizeChange::AdjustFixed(-50));
    f.double_roundtrip(id);

    // The top window shrinks in response, the bottom remains as is.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.ack_last_and_commit();
    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    // This starts the resize animation for the top window and the Y move for the bottom.
    f.roundtrip(id);

    // No time had passed yet, so we're at the initial state.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // Advance the time halfway.
    set_time(f.niri(), Duration::from_millis(500));
    f.niri().advance_animations();

    // Top window is half-resized at 75 px tall, bottom window is at y=75 matching it.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  75 at x:  0 y:  0
    200 × 200 at x:  0 y: 75
    ");

    // Advance the time to completion.
    set_time(f.niri(), Duration::from_millis(1000));
    f.niri().advance_animations();

    // Final state at 50 px.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_clientside_height_change_doesnt_animate() {
    let (mut f, id, surface1, _surface2) = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // The top window shrinks by itself, without a niri-issued resize.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.commit();

    // This does not start any animations.
    f.roundtrip(id);

    // No time had passed yet, but we are at the final state right away.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_resize_animation_starts_from_what_was_on_screen() {
    const FRAME: [u8; 3] = [0, 0, 255];
    const CONTENT: [u8; 3] = [0, 255, 0];

    let mut f = set_up();
    let id = f.add_client();

    // Like Firefox, the window draws its contents on a desynchronized subsurface.
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let (content, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_desync();

    let resize = |f: &mut Fixture, w: i32| {
        let client = f.client(id);
        // The contents change size first, and only then the window acks the configure.
        client.state.attach_shm_buffer(&content, w, 1080, CONTENT);
        content.commit();
        client.state.attach_shm_buffer(&surface, w, 1080, FRAME);
        let window = client.window(&surface);
        window.set_size(w as u16, 1080);
        window.ack_last_and_commit();
        f.roundtrip(id);
    };

    resize(&mut f, 960);
    f.double_roundtrip(id);
    set_time(f.niri(), Duration::ZERO);
    f.niri_complete_animations();
    render_output(&mut f, 1);

    f.niri().layout.toggle_full_width();
    f.double_roundtrip(id);
    resize(&mut f, 1920);
    f.niri_complete_animations();
    let pixels = render_output(&mut f, 1);
    assert_eq!(pixel(&pixels, 1440, 540), CONTENT);

    // Back to half width. At the start of the animation, the window should look the way it did
    // before the resize, not like the full-size frame with the half-size contents.
    f.niri().layout.toggle_full_width();
    f.double_roundtrip(id);
    resize(&mut f, 960);
    let pixels = render_output(&mut f, 1);
    assert_eq!(pixel(&pixels, 480, 540), CONTENT);
    assert_eq!(pixel(&pixels, 1440, 540), CONTENT);
}

#[test]
fn egl_resize_animation_off_takes_no_configure_snapshot() {
    for (window_resize_off, all_off) in [(false, false), (true, false), (false, true)] {
        let mut f = set_up_with(|config| {
            config.animations.window_resize.anim.off = window_resize_off;
            config.animations.off = all_off;
        });
        let id = f.add_client();

        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);
        let client = f.client(id);
        client
            .state
            .attach_shm_buffer(&surface, 960, 1080, [0, 0, 255]);
        client.window(&surface).ack_last_and_commit();
        f.double_roundtrip(id);
        render_output(&mut f, 1);

        f.niri().layout.toggle_full_width();
        f.double_roundtrip(id);

        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        let off = window_resize_off || all_off;
        assert_eq!(mapped.has_configure_snapshot(), !off);
    }
}

#[test]
fn egl_resize_animation_after_moving_to_output_with_other_scale() {
    const FRAME: [u8; 3] = [0, 0, 255];
    const CONTENT: [u8; 3] = [0, 255, 0];

    // The window either moves and then gets an animated configure, or it moves before it answers
    // an animated configure that it got on the first output.
    for resize_before_move in [false, true] {
        let mut f = set_up_with(|config| {
            let outputs = Config::parse_mem(r#"output "headless-2" { scale 2; }"#).unwrap();
            config.outputs = outputs.outputs;
        });
        f.add_output(2, (1920, 1080));
        let id = f.add_client();

        // The left half of the window is the contents, the right half is the frame.
        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);
        let (content, _subsurface) = f.client(id).state.create_subsurface(&surface);

        let draw = |f: &mut Fixture, w: i32, h: i32| {
            let client = f.client(id);
            client.state.attach_shm_buffer(&content, w / 2, h, CONTENT);
            content.commit();
            client.state.attach_shm_buffer(&surface, w, h, FRAME);
            client.window(&surface).ack_last_and_commit();
            f.roundtrip(id);
        };

        draw(&mut f, 960, 1080);
        f.double_roundtrip(id);
        set_time(f.niri(), Duration::ZERO);
        f.niri_complete_animations();
        render_output(&mut f, 1);

        if resize_before_move {
            f.niri().layout.toggle_full_width();
            f.double_roundtrip(id);
        }

        // Move the window to the scale-2 output, where it becomes 480×540. That output doesn't
        // render before the window answers.
        let output = f.niri_output(2);
        f.niri()
            .layout
            .move_to_output(None, &output, None, ActivateWindow::Smart);
        f.double_roundtrip(id);
        draw(&mut f, 480, 540);

        // At the start of the animation, the window is still 960 logical pixels wide, which is
        // all of output 2, so its left half is the contents.
        let pixels = render_output(&mut f, 2);
        assert_eq!(pixel(&pixels, 720, 270), CONTENT, "{resize_before_move}");
        assert_eq!(pixel(&pixels, 1200, 270), FRAME, "{resize_before_move}");
    }
}
