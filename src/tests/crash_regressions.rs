use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

use super::*;
use crate::tests::client::LayerConfigureProps;

#[test]
fn layer_on_removed_output_is_forgotten() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
    let id = f.add_client();

    f.double_roundtrip(id);
    let wl_output = f.client(id).output("headless-2");
    let layer = f
        .client(id)
        .create_layer(Some(&wl_output), Layer::Top, "bar");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(100, 50);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(f.niri().mapped_layer_surfaces.len(), 1);

    let output = f.niri_output(2);
    f.niri().remove_output(&output);
    f.double_roundtrip(id);

    // React to `closed` like waybar does: destroy the layer surface.
    let layer = f.client(id).layer(&surface);
    // (closed event was sent by remove_output)
    layer.layer_surface.destroy();
    layer.surface.destroy();
    f.double_roundtrip(id);

    let left = f.niri().mapped_layer_surfaces.len();
    assert_eq!(left, 0);
}

#[test]
fn clipboard_writes_are_bounded() {
    use std::sync::Arc;
    use std::time::Duration;

    use smithay::reexports::rustix::pipe::pipe;
    use smithay::wayland::selection::{SelectionHandler, SelectionTarget};

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let count = || std::fs::read_dir("/proc/self/task").unwrap().count();
    let before = count();

    // A 1 MiB "screenshot" in our clipboard, bigger than a pipe buffer.
    let data: Arc<[u8]> = Arc::from(vec![0u8; 1 << 20].into_boxed_slice());
    let mut readers = Vec::new();
    for _ in 0..200 {
        // What a client does with wl_data_offer.receive: hand us a pipe, then never read it.
        let (r, w) = pipe().unwrap();
        readers.push(r);
        let seat = f.niri().seat.clone();
        f.niri_state().send_selection(
            SelectionTarget::Clipboard,
            "image/png".into(),
            w,
            seat,
            &data,
        );
    }
    std::thread::sleep(Duration::from_millis(500));

    let stuck = count() - before;
    assert!(stuck < 50, "{stuck} threads stuck writing clipboard data");
}

#[test]
fn assign_workspace_to_removed_output() {
    use crate::protocols::ext_workspace::ExtWorkspaceHandler;

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));

    let ws_id = f.niri().layout.active_workspace().unwrap().id();
    // ext-workspace keeps only a WeakOutput in the pending Assign action; it still upgrades
    // because the disabled wl_output global holds the Output for 10 more seconds.
    let output = f.niri_output(2);
    f.niri().remove_output(&output);

    // What the queued Assign action does on the client's commit.
    f.niri_state().assign_workspace(ws_id, output);
}

#[test]
fn mru_click_with_pointer_off_all_outputs() {
    use wayland_client::protocol::wl_pointer;

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
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

    // Pointer on output 2, then output 2 is unplugged.
    let output2 = f.niri_output(2);
    let geo = f.niri().global_space.output_geometry(&output2).unwrap();
    let pos = smithay::utils::Point::from((geo.loc.x as f64 + 100., geo.loc.y as f64 + 100.));
    f.niri().seat.get_pointer().unwrap().set_location(pos);
    f.niri().remove_output(&output2);
    f.double_roundtrip(id);

    // Alt-Tab, then click without moving.
    f.niri_state().do_action(
        niri_config::Action::MruAdvance {
            direction: niri_config::MruDirection::Forward,
            scope: None,
            filter: None,
        },
        false,
    );
    assert!(
        f.niri().window_mru_ui.output().is_some(),
        "MRU did not open"
    );

    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let vp = manager.create_virtual_pointer(None, &client.qh, ());
    vp.button(0, 0x110, wl_pointer::ButtonState::Pressed);
    vp.frame();
    f.double_roundtrip(id);
}

#[test]
fn huge_output_position_does_not_overflow() {
    let config =
        niri_config::Config::parse_mem(r#"output "headless-1" { position x=2147483000 y=0; }"#)
            .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    // A second output with no configured position gets auto-placed to the right.
    f.add_output(2, (1280, 720));
}

#[test]
fn name_first_workspace_on_unfocused_monitor() {
    use niri_config::WorkspaceReference;

    let config = niri_config::Config::parse_mem("layout { empty-workspace-above-first; }").unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));

    // Open a window on output 2.
    f.niri_focus_output(2);
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

    // Focus output 1, then name output 2's empty top workspace by id (JSON IPC can do this).
    f.niri_focus_output(1);
    let out2 = f.niri_output(2);
    let ws_id = f
        .niri()
        .layout
        .workspaces()
        .find(|(mon, idx, _)| mon.is_some_and(|m| *m.output() == out2) && *idx == 0)
        .map(|(_, _, ws)| ws.id().get())
        .unwrap();
    f.niri()
        .layout
        .set_workspace_name(String::from("foo"), Some(WorkspaceReference::Id(ws_id)));

    // The window closes, then the user goes to "foo" on output 2.
    let window = f.client(id).window(&surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(id);
    f.niri_complete_animations();
    f.niri_focus_output(2);
    f.niri().layout.switch_workspace(0);
    f.niri_complete_animations();
}

#[test]
fn ipc_request_size_is_bounded() {
    use std::io::{ErrorKind, Read as _, Write as _};
    use std::os::unix::net::UnixStream;

    let mut f = Fixture::new();
    let (mut client, server) = UnixStream::pair().unwrap();
    client.set_nonblocking(true).unwrap();
    crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

    // A normal request still gets its reply.
    client.write_all(b"\"Version\"\n").unwrap();
    let mut reply = Vec::new();
    for _ in 0..100 {
        f.state.server.dispatch();
        let mut buf = [0; 256];
        match client.read(&mut buf) {
            Ok(n) => reply.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => (),
            Err(err) => panic!("error reading reply: {err:?}"),
        }
        if reply.ends_with(b"\n") {
            break;
        }
    }
    assert!(reply.starts_with(b"{\"Ok\":{\"Version\""), "{reply:?}");

    // A request that never ends, like `cat /dev/zero` into the socket. niri must drop the client
    // rather than buffer all of it.
    let chunk = [b'a'; 64 * 1024];
    let mut sent = 0;
    let mut dropped = false;
    for _ in 0..1000 {
        if sent >= 8 << 20 {
            break;
        }
        match client.write(&chunk) {
            Ok(n) => sent += n,
            Err(err) if err.kind() == ErrorKind::WouldBlock => (),
            Err(_) => {
                dropped = true;
                break;
            }
        }
        f.state.server.dispatch();

        // Closing with our data still unread shows up as EOF or as a reset.
        let mut buf = [0; 64];
        match client.read(&mut buf) {
            Err(err) if err.kind() == ErrorKind::WouldBlock => (),
            Ok(n @ 1..) => panic!("unexpected {n}-byte reply"),
            Ok(0) | Err(_) => {
                dropped = true;
                break;
            }
        }
    }
    assert!(
        dropped,
        "IPC client still connected after sending {sent} bytes"
    );
}
