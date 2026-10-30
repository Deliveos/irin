use std::os::fd::{AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use std::{io, thread};

use calloop::generic::{Generic, NoIoDrop};
use calloop::ping::{make_ping, Ping};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, LoopHandle, Mode, PostAction, Readiness, RegistrationToken};
use smithay::reexports::rustix::io::{fcntl_setfd, FdFlags};

use crate::niri::State;
use crate::utils::expand_home;
use crate::utils::xwayland::X11Connection;

/// How long the on-demand activation test may take.
///
/// The test runs on the main thread, at startup and on a config reload that changes the
/// `xwayland-satellite` section while the integration isn't running, and normally takes a few
/// milliseconds. A configured path to a program that doesn't exit would otherwise freeze niri.
const TEST_ONDEMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait before watching the X11 sockets again after an error.
///
/// Such errors usually mean that niri is out of file descriptors. Then it can't accept a pending
/// connection either, and the connection would trigger the watch again right away.
const RETRY_WATCH_DELAY: Duration = Duration::from_secs(1);

pub struct Satellite {
    x11: X11Connection,
    abstract_token: Option<RegistrationToken>,
    unix_token: Option<RegistrationToken>,
    /// Sets up the watch again, after xwayland-satellite exits.
    setup_watch_ping: Ping,
}

impl Satellite {
    pub fn display_name(&self) -> &str {
        &self.x11.display_name
    }
}

pub fn setup(state: &mut State) {
    if state.niri.satellite.is_some() {
        return;
    }

    let config = state.niri.config.borrow();
    let xwls_config = &config.xwayland_satellite;
    if xwls_config.off {
        return;
    }

    if !test_ondemand(&xwls_config.path) {
        return;
    }
    drop(config);

    // A ping rather than a calloop channel, which panics when it can't create its ping, for
    // example when out of file descriptors.
    let (setup_watch_ping, source) = match make_ping() {
        Ok(x) => x,
        Err(err) => {
            warn!("error creating a ping, disabling xwayland-satellite integration: {err:?}");
            return;
        }
    };

    let x11 = match super::setup_connection() {
        Ok(x11) => x11,
        Err(err) => {
            warn!("error opening X11 sockets, disabling xwayland-satellite integration: {err:?}");
            return;
        }
    };

    let event_loop = &state.niri.event_loop;
    let res = event_loop.insert_source(source, |(), _, state| setup_watch(state, true));
    if let Err(err) = res {
        warn!("error inserting a ping, disabling xwayland-satellite integration: {err:?}");
        return;
    }

    state.niri.satellite = Some(Satellite {
        x11,
        abstract_token: None,
        unix_token: None,
        setup_watch_ping,
    });

    setup_watch(state, true);
}

fn test_ondemand(path: &str) -> bool {
    let _span = tracy_client::span!("satellite::test_ondemand");

    // Expand `~` at the start.
    let mut path = Path::new(path);
    let expanded = expand_home(path);
    match &expanded {
        Ok(Some(expanded)) => path = expanded.as_ref(),
        Ok(None) => (),
        Err(err) => {
            warn!("error expanding ~: {err:?}");
        }
    }

    let mut process = Command::new(path);
    process
        .args([":0", "--test-listenfd-support"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("DISPLAY")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE");

    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(err) => {
            warn!("error spawning xwayland-satellite at {path:?}, disabling integration: {err}");
            return false;
        }
    };

    let status = match wait_with_timeout(&mut child, TEST_ONDEMAND_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            warn!(
                "xwayland-satellite at {path:?} didn't exit within {TEST_ONDEMAND_TIMEOUT:?} \
                 when testing on-demand activation, disabling integration"
            );
            return false;
        }
        Err(err) => {
            warn!("error waiting for xwayland-satellite, disabling integration: {err}");
            return false;
        }
    };

    if !status.success() {
        warn!("xwayland-satellite doesn't support on-demand activation, disabling integration");
        return false;
    }

    true
}

/// Waits for the child to exit, or kills it after the timeout and returns `None`.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }

        thread::sleep(Duration::from_millis(1));
    }
}

// When xwayland-satellite fails to start and accept a connection on the socket, the socket will
// keep triggering our event source, even after the X11 client quits, resulting in a busyloop of
// trying to start xwayland-satellite. This function will clear out (accept and drop) all pending
// connections on the socket before registering a new event source, working around this problem.
// When the problem happens, it's very likely that xwayland-satellite won't be able to accept the
// pending client (since it had just failed to do so), so it's fine to drop the connections.
fn clear_out_pending_connections(fd: OwnedFd) -> OwnedFd {
    let listener = UnixListener::from(fd);

    if let Err(err) = listener.set_nonblocking(true) {
        warn!("error setting X11 socket to nonblocking: {err:?}");
        return OwnedFd::from(listener);
    }

    while listener.accept().is_ok() {}

    if let Err(err) = listener.set_nonblocking(false) {
        warn!("error setting X11 socket to blocking: {err:?}");
    }

    OwnedFd::from(listener)
}

/// Watches the X11 sockets, spawning xwayland-satellite when a client connects.
///
/// With `clear_pending`, first drops the connections that are already pending (see
/// [`clear_out_pending_connections`]).
fn setup_watch(state: &mut State, clear_pending: bool) {
    let Some(satellite) = state.niri.satellite.as_mut() else {
        return;
    };

    let event_loop = &state.niri.event_loop;

    if let Some(token) = satellite.abstract_token.take() {
        error!("abstract_token must be None in setup_watch()");
        event_loop.remove(token);
    }
    if let Some(token) = satellite.unix_token.take() {
        error!("unix_token must be None in setup_watch()");
        event_loop.remove(token);
    }

    if let Some(fd) = &satellite.x11.abstract_fd {
        satellite.abstract_token =
            watch_socket(event_loop, fd, clear_pending, move |_, _, state| {
                if let Some(satellite) = &mut state.niri.satellite {
                    // Remove the other source.
                    if let Some(token) = satellite.unix_token.take() {
                        state.niri.event_loop.remove(token);
                    }
                    // Clear this source.
                    satellite.abstract_token = None;

                    debug!("connection to X11 abstract socket; spawning xwayland-satellite");
                    let path = state.niri.config.borrow().xwayland_satellite.path.clone();
                    if spawn(path, satellite).is_none() {
                        setup_watch_later(&state.niri.event_loop);
                    }
                }
                Ok(PostAction::Remove)
            });
    }

    let unix_fd = &satellite.x11.unix_fd;
    satellite.unix_token = watch_socket(event_loop, unix_fd, clear_pending, move |_, _, state| {
        if let Some(satellite) = &mut state.niri.satellite {
            // Remove the other source.
            if let Some(token) = satellite.abstract_token.take() {
                state.niri.event_loop.remove(token);
            }
            // Clear this source.
            satellite.unix_token = None;

            debug!("connection to X11 unix socket; spawning xwayland-satellite");
            let path = state.niri.config.borrow().xwayland_satellite.path.clone();
            if spawn(path, satellite).is_none() {
                setup_watch_later(&state.niri.event_loop);
            }
        }
        Ok(PostAction::Remove)
    });

    // Watch both sockets or neither, so that a client of the socket that isn't watched doesn't
    // wait until another client connects to the other one.
    let abstract_failed = satellite.x11.abstract_fd.is_some() && satellite.abstract_token.is_none();
    if abstract_failed || satellite.unix_token.is_none() {
        if let Some(token) = satellite.abstract_token.take() {
            event_loop.remove(token);
        }
        if let Some(token) = satellite.unix_token.take() {
            event_loop.remove(token);
        }
        setup_watch_later(event_loop);
    }
}

/// Watches a copy of an X11 socket, calling `callback` when a client connects.
///
/// Returns `None` and logs a warning on errors.
fn watch_socket<F>(
    event_loop: &LoopHandle<'static, State>,
    fd: &OwnedFd,
    clear_pending: bool,
    callback: F,
) -> Option<RegistrationToken>
where
    F: FnMut(Readiness, &mut NoIoDrop<OwnedFd>, &mut State) -> io::Result<PostAction> + 'static,
{
    let fd = match fd.try_clone() {
        Ok(fd) => fd,
        Err(err) => {
            warn!("error duplicating an X11 socket: {err:?}");
            return None;
        }
    };
    let fd = if clear_pending {
        clear_out_pending_connections(fd)
    } else {
        fd
    };
    let source = Generic::new(fd, Interest::READ, Mode::Level);
    match event_loop.insert_source(source, callback) {
        Ok(token) => Some(token),
        Err(err) => {
            warn!("error watching an X11 socket: {:?}", err.error);
            None
        }
    }
}

/// Sets up the X11 socket watch after [`RETRY_WATCH_DELAY`].
///
/// Keeps the pending connections. After a failed spawn, xwayland-satellite hasn't tried to accept
/// them, so the watch spawns it for them. When setting up the watch failed after
/// xwayland-satellite exited, a socket that couldn't be copied wasn't cleared, so the connections
/// that xwayland-satellite left there spawn it again.
fn setup_watch_later(event_loop: &LoopHandle<'static, State>) {
    debug!("watching the X11 sockets again in {RETRY_WATCH_DELAY:?}");
    let timer = Timer::from_duration(RETRY_WATCH_DELAY);
    let res = event_loop.insert_source(timer, |_, _, state| {
        setup_watch(state, false);
        TimeoutAction::Drop
    });
    if let Err(err) = res {
        error!("error inserting the timer to watch the X11 sockets again: {err:?}");
    }
}

/// Spawns xwayland-satellite on a thread, which sets up the watch again when it exits.
///
/// Returns `None` if it couldn't copy the X11 sockets, then the caller sets up the watch again.
fn spawn(path: String, xwl: &Satellite) -> Option<()> {
    let _span = tracy_client::span!("satellite::spawn");

    let clone_fds = || -> io::Result<_> {
        let abstract_fd = xwl.x11.abstract_fd.as_ref().map(OwnedFd::try_clone);
        Ok((abstract_fd.transpose()?, xwl.x11.unix_fd.try_clone()?))
    };
    let (abstract_fd, unix_fd) = match clone_fds() {
        Ok(fds) => fds,
        Err(err) => {
            warn!("error duplicating the X11 sockets for xwayland-satellite: {err:?}");
            return None;
        }
    };
    let setup_watch_ping = xwl.setup_watch_ping.clone();

    // Expand `~` at the start.
    let mut path = PathBuf::from(path);
    let expanded = expand_home(&path);
    match expanded {
        Ok(Some(expanded)) => path = expanded,
        Ok(None) => (),
        Err(err) => {
            warn!("error expanding ~: {err:?}");
        }
    }

    let mut process = Command::new(&path);
    process.arg(&xwl.x11.display_name).env_remove("DISPLAY");

    // We don't want it spamming the niri output.
    process
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE");
    process
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    unsafe { process.pre_exec(crate::utils::signals::unblock_all) };

    // Spawning and waiting takes some milliseconds, so do it in a thread.
    let res = thread::Builder::new()
        .name("Xwl-s Spawner".to_owned())
        .spawn(move || {
            spawn_and_wait(&path, process, abstract_fd, unix_fd);

            // Once xwayland-satellite crashes or fails to spawn, re-establish our X11 socket watch
            // to try again next time.
            setup_watch_ping.ping();
        });

    if let Err(err) = res {
        warn!("error spawning a thread to spawn xwayland-satellite: {err:?}");
        xwl.setup_watch_ping.ping();
    }

    Some(())
}

fn spawn_and_wait(
    path: &Path,
    mut process: Command,
    abstract_fd: Option<OwnedFd>,
    unix_fd: OwnedFd,
) {
    let abstract_raw = abstract_fd.as_ref().map(|fd| fd.as_raw_fd());
    let unix_raw = unix_fd.as_raw_fd();

    process.arg("-listenfd").arg(unix_raw.to_string());

    if let Some(abstract_raw) = abstract_raw {
        process.arg("-listenfd").arg(abstract_raw.to_string());
    }

    unsafe {
        process.pre_exec(move || {
            // We're about to exec xwl-s; perfect time to clear CLOEXEC on the file descriptors
            // that we want to pass it.

            // We're not dropping these until after spawn().
            let unix_fd = BorrowedFd::borrow_raw(unix_raw);
            fcntl_setfd(unix_fd, FdFlags::empty())?;

            if let Some(abstract_raw) = abstract_raw {
                let abstract_fd = BorrowedFd::borrow_raw(abstract_raw);
                fcntl_setfd(abstract_fd, FdFlags::empty())?;
            }

            Ok(())
        })
    };

    let mut child = {
        let _span = tracy_client::span!();
        match process.spawn() {
            Ok(child) => child,
            Err(err) => {
                warn!("error spawning {path:?}: {err:?}");
                return;
            }
        }
    };

    // The process spawned, we can drop our fds.
    drop(abstract_fd);
    drop(unix_fd);

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            warn!("error waiting for xwayland-satellite: {err:?}");
            return;
        }
    };

    // This is most likely a crash, hence warn!().
    warn!("xwayland-satellite exited with: {status}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_with_timeout_kills_a_child_that_doesnt_exit() {
        let mut child = Command::new("true").spawn().unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(30));
        assert!(status.unwrap().unwrap().success());

        let mut child = Command::new("sleep").arg("1000").spawn().unwrap();
        let start = Instant::now();
        let status = wait_with_timeout(&mut child, Duration::from_millis(100));
        assert!(status.unwrap().is_none());
        assert!(start.elapsed() < Duration::from_secs(30));
        // The child was killed and reaped.
        assert!(child.try_wait().unwrap().is_some());
    }
}
