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
fn load_config_file_checks_the_path() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use niri_ipc::socket::Socket;
    use niri_ipc::{Action, Request, Response};

    let mut f = Fixture::new();
    let dir = std::env::temp_dir().join(format!("niri-test-ipc-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config.kdl");
    std::fs::write(&file, "").unwrap();
    let path = dir.join("socket");
    let listener = UnixListener::bind(&path).unwrap();

    let send = |f: &mut Fixture, config_path: &std::path::Path| {
        let request = Request::Action(Action::LoadConfigFile {
            path: Some(config_path.to_str().unwrap().to_owned()),
        });
        let (tx, rx) = mpsc::channel();
        let socket_path = path.clone();
        std::thread::spawn(move || {
            let reply = Socket::connect_to(&socket_path).and_then(|mut s| s.send(request));
            let _ = tx.send(reply);
        });
        let (server, _) = listener.accept().unwrap();
        crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

        // The check runs on a thread, so keep dispatching until the reply comes.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            f.dispatch();
            if let Ok(reply) = rx.recv_timeout(Duration::from_millis(1)) {
                return reply.unwrap();
            }
            assert!(Instant::now() < deadline, "no reply from niri");
        }
    };

    let missing = dir.join("missing.kdl");
    let err = send(&mut f, &missing).unwrap_err();
    assert!(err.starts_with("path does not point to a file"), "{err}");

    let err = send(&mut f, &dir).unwrap_err();
    assert!(err.starts_with("path does not point to a file"), "{err}");

    let reply = send(&mut f, &file).unwrap();
    assert!(matches!(reply, Response::Handled), "{reply:?}");

    std::fs::remove_dir_all(&dir).unwrap();
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

#[test]
fn event_stream_client_gets_a_burst_of_events() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use niri_ipc::socket::Socket;
    use niri_ipc::{Event, Request};

    const WINDOWS: usize = 100;

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let mut surfaces = Vec::new();
    for i in 0..WINDOWS {
        let surface = create_unmapped(&mut f, id, &format!("a{i}"));
        commit_buffer(&mut f, id, &surface);
        f.double_roundtrip(id);
        surfaces.push(surface);
    }

    let dir = std::env::temp_dir().join(format!("niri-test-ipc-burst-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("socket");
    let listener = UnixListener::bind(&path).unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut socket = Socket::connect_to(&path).unwrap();
        socket.send(Request::EventStream).unwrap().unwrap();
        let mut read_event = socket.read_events();
        // Stops when niri disconnects the client.
        while let Ok(event) = read_event() {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    let (server, _) = listener.accept().unwrap();
    crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

    // Reads events until one matches, while dispatching niri.
    let wait_for = |f: &mut Fixture, pred: &mut dyn FnMut(&Event) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            f.dispatch();
            match rx.recv_timeout(Duration::from_millis(1)) {
                Ok(event) if pred(&event) => return,
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("niri disconnected the event stream client")
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for an event");
        }
    };

    wait_for(
        &mut f,
        &mut |event| matches!(event, Event::WindowsChanged { windows } if windows.len() == WINDOWS),
    );

    // Change every title in one refresh, which makes one event per window, more than the old
    // limit of 64 queued events.
    for (i, surface) in surfaces.iter().enumerate() {
        f.client(id).window(surface).set_title(&format!("b{i}"));
    }
    f.roundtrip(id);

    let mut changed = std::collections::HashSet::new();
    wait_for(&mut f, &mut |event| {
        if let Event::WindowOpenedOrChanged { window } = event {
            changed.insert(window.title.clone().unwrap());
        }
        changed.len() == WINDOWS
    });
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn event_stream_client_that_doesnt_read_is_dropped() {
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    use niri_ipc::socket::Socket;
    use niri_ipc::Request;

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let mut surfaces = Vec::new();
    for i in 0..10 {
        let surface = create_unmapped(&mut f, id, &format!("a{i}"));
        commit_buffer(&mut f, id, &surface);
        f.double_roundtrip(id);
        surfaces.push(surface);
    }

    let dir = std::env::temp_dir().join(format!("niri-test-ipc-stalled-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("socket");
    let listener = UnixListener::bind(&path).unwrap();

    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut socket = Socket::connect_to(&path).unwrap();
        socket.send(Request::EventStream).unwrap().unwrap();
        // Keep the socket open without reading from it.
        let _ = done_rx.recv();
        drop(socket);
    });
    let (server, _) = listener.accept().unwrap();
    crate::ipc::server::on_new_ipc_client(f.niri_state(), server);

    let stream_count = |f: &mut Fixture| f.niri().ipc_server.as_ref().unwrap().event_stream_count();
    while stream_count(&mut f) == 0 {
        f.dispatch();
    }

    // Each round makes 10 events of over 1000 bytes. niri should drop the client after its
    // socket buffer and the 1 MiB queue fill up.
    let long = "x".repeat(1000);
    let mut rounds = 0;
    while stream_count(&mut f) == 1 {
        assert!(rounds < 2000, "niri kept the client that doesn't read");
        for (i, surface) in surfaces.iter().enumerate() {
            f.client(id)
                .window(surface)
                .set_title(&format!("{rounds} {i} {long}"));
        }
        f.roundtrip(id);
        f.dispatch();
        rounds += 1;
    }

    drop(done_tx);
    std::fs::remove_dir_all(&dir).unwrap();
}
