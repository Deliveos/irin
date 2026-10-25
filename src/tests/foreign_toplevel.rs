use client::ClientId;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::utils::with_toplevel_role;

fn open_window(f: &mut Fixture, id: ClientId, title: &str) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_title(title);
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

fn close_window(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.client(id).window(surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(id);
}

/// Titles of the toplevels announced to the client, and whether each is closed.
fn toplevels(f: &mut Fixture, id: ClientId) -> Vec<(String, bool)> {
    f.client(id)
        .state
        .foreign_toplevels
        .iter()
        .map(|t| (t.title.clone().unwrap_or_default(), t.closed))
        .collect()
}

#[test]
fn closing_windows_sends_closed_once() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    f.roundtrip(id);
    assert!(f.client(id).state.foreign_toplevel_list.is_some());

    let a = open_window(&mut f, id, "a");
    let b = open_window(&mut f, id, "b");
    let c = open_window(&mut f, id, "c");
    let open = |t: &str| (t.to_owned(), false);
    let closed = |t: &str| (t.to_owned(), true);
    assert_eq!(toplevels(&mut f, id), [open("a"), open("b"), open("c")]);

    // The focused window goes through a separate path, and it must stay open across refreshes.
    let focused =
        f.niri().layout.focus().map(|m| {
            with_toplevel_role(m.toplevel(), |role| role.title.clone().unwrap_or_default())
        });
    assert_eq!(focused.as_deref(), Some("c"));
    f.double_roundtrip(id);
    assert_eq!(toplevels(&mut f, id), [open("a"), open("b"), open("c")]);

    // Closing a window that isn't focused.
    close_window(&mut f, id, &a);
    assert_eq!(toplevels(&mut f, id), [closed("a"), open("b"), open("c")]);

    // Closing the focused window, which moves the focus to another one.
    close_window(&mut f, id, &c);
    assert_eq!(toplevels(&mut f, id), [closed("a"), open("b"), closed("c")]);

    close_window(&mut f, id, &b);
    assert_eq!(
        toplevels(&mut f, id),
        [closed("a"), closed("b"), closed("c")]
    );
}
