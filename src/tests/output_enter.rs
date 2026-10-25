use client::ClientId;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

fn entered_outputs(f: &mut Fixture, id: ClientId, surface: &WlSurface) -> usize {
    f.client(id).state.surfaces[surface].entered_outputs.len()
}

#[test]
fn new_subsurfaces_and_popups_enter_the_output() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &surface), 1);

    // Let some event loop iterations pass with nothing changing.
    f.double_roundtrip(id);

    // A subsurface added after mapping, which appears with the parent's commit.
    let (subsurface, _) = f.client(id).state.create_subsurface(&surface);
    f.client(id).window(&surface).commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &subsurface), 1);

    f.double_roundtrip(id);

    // A desynchronized subsurface, which appears with its own commit.
    let (desync, desync_subsurface) = f.client(id).state.create_subsurface(&surface);
    desync_subsurface.set_desync();
    f.client(id).state.attach_pixel_buffer(&desync);
    desync.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &desync), 1);

    f.double_roundtrip(id);

    // A popup, which maps with its own commit.
    let xdg_surface = f.client(id).window(&surface).xdg_surface.clone();
    let (popup, _, _) = f.client(id).state.create_popup(&xdg_surface);
    f.roundtrip(id);
    f.client(id).state.attach_pixel_buffer(&popup);
    popup.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &popup), 1);
}
