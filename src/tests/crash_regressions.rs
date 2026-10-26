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

    // Count only the writer threads, since other tests start threads in parallel. The name is
    // "Selection Writer", cut to the 15 bytes that Linux keeps.
    let writers = || {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
            .filter(|comm| comm.trim_end() == "Selection Write")
            .count()
    };

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

    let stuck = writers();
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

/// Writes `data` to an IPC client socket while dispatching niri. Returns what niri replied, up to
/// the first full line, and whether it closed the connection.
fn ipc_exchange(
    f: &mut Fixture,
    client: &mut std::os::unix::net::UnixStream,
    mut data: &[u8],
) -> (Vec<u8>, bool) {
    use std::io::{ErrorKind, Read as _, Write as _};

    let mut reply = Vec::new();
    let mut closed = false;
    for _ in 0..10_000 {
        if !data.is_empty() {
            let len = data.len().min(64 * 1024);
            match client.write(&data[..len]) {
                Ok(n) => data = &data[n..],
                Err(err) if err.kind() == ErrorKind::WouldBlock => (),
                // niri closed the connection; still read what it sent before that.
                Err(_) => {
                    closed = true;
                    data = &[];
                }
            }
        }
        f.state.server.dispatch();

        let mut buf = [0; 4096];
        match client.read(&mut buf) {
            Ok(0) => return (reply, true),
            Ok(n) => reply.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => (),
            Err(_) => return (reply, true),
        }
        if data.is_empty() && reply.ends_with(b"\n") {
            return (reply, closed);
        }
    }
    panic!("no reply from niri");
}

#[test]
fn ipc_request_size_is_bounded() {
    use std::os::unix::net::UnixStream;

    let mut f = Fixture::new();
    let (mut client, server) = UnixStream::pair().unwrap();
    client.set_nonblocking(true).unwrap();
    crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

    let version_reply = |reply: &[u8]| reply.starts_with(br#"{"Ok":{"Version""#);

    // A normal request gets its reply.
    let (reply, closed) = ipc_exchange(&mut f, &mut client, b"\"Version\"\n");
    assert!(version_reply(&reply) && !closed, "{reply:?}");

    // So does a valid request over 1 MiB, like a spawn with a long argv.
    let mut request = vec![b' '; 2 << 20];
    request.extend_from_slice(b"\"Version\"\n");
    let (reply, closed) = ipc_exchange(&mut f, &mut client, &request);
    assert!(
        version_reply(&reply) && !closed,
        "{:?}",
        String::from_utf8_lossy(&reply)
    );

    // A request that never ends, like `cat /dev/zero` into the socket. niri must tell the client
    // and drop it rather than buffer all of it.
    let (reply, closed) = ipc_exchange(&mut f, &mut client, &vec![b'a'; 16 << 20]);
    assert!(closed, "IPC client still connected after a 16 MiB request");
    let reply = String::from_utf8_lossy(&reply);
    assert!(
        reply.starts_with(r#"{"Err":"request is longer than"#),
        "{reply}"
    );
}

#[test]
fn huge_popup_positioner_values() {
    use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::{
        self, ConstraintAdjustment, Gravity, XdgPositioner,
    };

    use crate::tests::client::PopupXdgSurface;

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

    // Positioner values that each overflowed somewhere in the popup placement math.
    let cases: [&dyn Fn(&XdgPositioner); 5] = [
        &|p| {
            p.set_anchor_rect(i32::MAX - 5, i32::MAX - 5, 10, 10);
            p.set_anchor(xdg_positioner::Anchor::BottomRight);
        },
        &|p| p.set_offset(i32::MAX, i32::MAX),
        &|p| {
            p.set_offset(i32::MIN, i32::MIN);
            p.set_gravity(Gravity::TopLeft);
        },
        &|p| {
            p.set_offset(i32::MIN, i32::MIN);
            p.set_constraint_adjustment(ConstraintAdjustment::all());
        },
        &|p| {
            p.set_offset(i32::MAX, i32::MAX);
            p.set_constraint_adjustment(ConstraintAdjustment::all());
        },
    ];

    // Each popup nests in the previous one, so that their positions add up too.
    let mut parent = f.client(id).window(&surface).xdg_surface.clone();
    for configure in cases {
        let state = &mut f.client(id).state;
        let qh = state.qh.clone();
        let xdg_wm_base = state.xdg_wm_base.clone().unwrap();
        let positioner = xdg_wm_base.create_positioner(&qh, ());
        positioner.set_size(10, 10);
        positioner.set_anchor_rect(0, 0, 1, 1);
        configure(&positioner);

        let surface = state.create_surface();
        let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &qh, PopupXdgSurface);
        let popup = xdg_surface.get_popup(Some(&parent), &positioner, &qh, ());
        surface.commit();
        f.roundtrip(id);

        popup.reposition(&positioner, 1);
        positioner.destroy();
        f.client(id).state.attach_pixel_buffer(&surface);
        surface.commit();
        f.double_roundtrip(id);

        parent = xdg_surface;
    }
}
