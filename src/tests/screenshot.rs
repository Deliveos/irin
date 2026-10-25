use niri_config::Config;

use super::*;

/// GL texture ids of the screenshot UI captures of the first output, for each render target.
fn capture_texture_ids(f: &mut Fixture) -> [u32; 3] {
    let state = f.niri_state();
    let niri = &state.niri;
    state
        .backend
        .with_primary_renderer(|renderer| {
            let (_, screenshot) = niri.capture_screenshots(renderer).next().unwrap();
            screenshot.map(|s| s.texture().tex_id())
        })
        .unwrap()
}

fn set_up(config: Config) -> Fixture {
    let mut f = Fixture::with_config(config);
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (640, 480));
    f
}

#[test]
fn egl_screenshot_targets_share_texture_without_block_out() {
    let mut f = set_up(Config::default());
    let [a, b, c] = capture_texture_ids(&mut f);
    assert!(a == b && b == c, "{a} {b} {c}");
}

#[test]
fn egl_screenshot_targets_render_separately_with_block_out() {
    let config = Config::parse_mem(r#"window-rule { block-out-from "screencast"; }"#).unwrap();
    let mut f = set_up(config);
    let [a, b, c] = capture_texture_ids(&mut f);
    assert!(a != b && b != c && a != c, "{a} {b} {c}");
}
