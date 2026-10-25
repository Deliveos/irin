use client::ClientId;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

/// Creates a window with the title and commits it without a buffer, so it's not mapped yet.
fn create_unmapped(f: &mut Fixture, id: ClientId, title: &str) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_title(title);
    window.commit();
    f.roundtrip(id);
    surface
}

/// Attaches a buffer to an unmapped window, mapping it on the next roundtrip.
fn commit_buffer(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.client(id).window(surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
}

fn destroy(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.client(id).window(surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
}

/// Titles of the windows in the IPC event stream state, sorted.
fn ipc_titles(f: &mut Fixture) -> Vec<String> {
    let server = f.niri().ipc_server.as_ref().unwrap();
    let state = server.event_stream_state();
    let mut titles: Vec<_> = state
        .windows
        .windows
        .values()
        .map(|win| win.title.clone().unwrap_or_default())
        .collect();
    titles.sort();
    titles
}

#[test]
fn ipc_tracks_opened_and_closed_windows() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let a = create_unmapped(&mut f, id, "a");
    let b = create_unmapped(&mut f, id, "b");
    let c = create_unmapped(&mut f, id, "c");
    for surface in [&a, &b, &c] {
        commit_buffer(&mut f, id, surface);
    }
    f.double_roundtrip(id);
    assert_eq!(ipc_titles(&mut f), ["a", "b", "c"]);

    destroy(&mut f, id, &b);
    f.double_roundtrip(id);
    assert_eq!(ipc_titles(&mut f), ["a", "c"]);

    // A window closes and another one opens in the same refresh, so the number of windows stays
    // the same.
    let d = create_unmapped(&mut f, id, "d");
    destroy(&mut f, id, &a);
    commit_buffer(&mut f, id, &d);
    f.roundtrip(id);
    assert_eq!(ipc_titles(&mut f), ["c", "d"]);
}

#[test]
fn socket_gets_reply_to_oversized_request() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use niri_ipc::socket::Socket;
    use niri_ipc::{Action, Request};

    let mut f = Fixture::new();
    let dir = std::env::temp_dir().join(format!("niri-test-ipc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("socket");
    let listener = UnixListener::bind(&path).unwrap();

    // Longer than niri's limit by more than a socket buffer holds, so niri closes the connection
    // while the client is still writing.
    let command = vec!["a".repeat(12 << 20)];
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let request = Request::Action(Action::Spawn { command });
        let reply = Socket::connect_to(&path).and_then(|mut socket| socket.send(request));
        let _ = tx.send(reply);
    });
    let (server, _) = listener.accept().unwrap();
    crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

    let deadline = Instant::now() + Duration::from_secs(30);
    let reply = loop {
        f.state.server.dispatch();
        if let Ok(reply) = rx.recv_timeout(Duration::from_millis(1)) {
            break reply;
        }
        assert!(Instant::now() < deadline, "no reply from niri");
    };
    std::fs::remove_dir_all(&dir).unwrap();

    let err = reply.unwrap().unwrap_err();
    assert!(err.starts_with("request is longer than"), "{err}");
}
