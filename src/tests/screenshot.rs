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

#[test]
fn screenshot_threads_are_limited() {
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::niri::MAX_SCREENSHOTS_IN_FLIGHT;

    let mut f = Fixture::new();
    let niri = f.niri();

    // Threads that wait until the test lets them finish.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
    let (done_tx, done_rx) = mpsc::channel();
    for _ in 0..MAX_SCREENSHOTS_IN_FLIGHT {
        let release_rx = release_rx.clone();
        let done_tx = done_tx.clone();
        niri.spawn_screenshot_thread(move || {
            let _ = release_rx.lock().unwrap().recv();
            let _ = done_tx.send(());
        })
        .unwrap();
    }

    // One more is refused, and so is the check before rendering one.
    assert!(niri.spawn_screenshot_thread(|| ()).is_err());
    assert!(niri.check_screenshots_in_flight().is_err());

    // Once they finish, screenshots work again.
    drop(release_tx);
    for _ in 0..MAX_SCREENSHOTS_IN_FLIGHT {
        done_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while niri.check_screenshots_in_flight().is_err() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    niri.spawn_screenshot_thread(|| ()).unwrap();
}
