use client::ClientId;
use insta::assert_snapshot;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::utils::with_toplevel_role;

fn open_window(f: &mut Fixture, id: ClientId) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

#[test]
fn idle_windows_keep_no_pending_configure_state() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let surface1 = open_window(&mut f, id);
    let surface2 = open_window(&mut f, id);

    // Ack everything and let a few event loop iterations pass.
    for surface in [&surface1, &surface2] {
        let window = f.client(id).window(surface);
        window.ack_last_and_commit();
        let _ = window.recent_configures();
    }
    for _ in 0..3 {
        f.double_roundtrip(id);
    }

    // Every iteration checks whether a configure is needed; with nothing changing, that must not
    // leave a pending state behind, or the next check clones and compares the whole state again.
    for (_, mapped) in f.niri().layout.windows() {
        let has_pending =
            with_toplevel_role(mapped.toplevel(), |role| role.server_pending.is_some());
        assert!(!has_pending, "idle window has a pending configure state");
    }

    // Changes still go out: focusing the first window activates it and deactivates the second.
    f.niri().layout.focus_left();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface1);
    assert_snapshot!(window.format_recent_configures(), @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]");
    let window = f.client(id).window(&surface2);
    assert_snapshot!(window.format_recent_configures(), @"size: 936 × 1048, bounds: 1888 × 1048, states: []");
}
