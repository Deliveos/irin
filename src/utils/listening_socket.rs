//! Event sources for listening sockets that survive accept() errors.
//!
//! calloop returns an event source's error from the event loop dispatch, and niri unwraps the
//! result of running the event loop, so an accept() error on a listening socket (for example
//! EMFILE when niri runs out of file descriptors) would end the session.

use std::cell::Cell;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::{
    EventSource, InsertError, LoopHandle, Poll, PostAction, Readiness, RegistrationToken, Token,
    TokenFactory,
};

/// How long to stop accepting connections after accept() fails.
///
/// The sockets are level-triggered, so polling them again right away would spin for as long as
/// the error lasts.
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_secs(1);

/// What to do when accepting a connection fails with an error other than a transient one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnAcceptError {
    /// Stop accepting for a while, then try again.
    ///
    /// Only for sources with a single fd: enabling a source registers its fds again, and a source
    /// with several fds can be left half registered when that fails.
    Pause,
    /// Remove the event source.
    ///
    /// For sockets that come from clients. smithay's security-context source has two fds and
    /// doesn't undo a partial registration, so it can't be paused safely.
    Remove,
}

/// Returns a strong handle of the event loop, if the loop still exists.
///
/// calloop doesn't export the type of its weak loop handle, so the weak handle lives in here.
type UpgradeLoop<Data> = Rc<dyn Fn() -> Option<LoopHandle<'static, Data>>>;

/// Wraps the event source of a listening socket so that its errors don't end the event loop.
pub struct ListeningSocket<S, Data> {
    inner: S,
    name: String,
    on_error: OnAcceptError,
    // Weak, because this source lives in the event loop: a strong handle would keep the loop
    // alive after it is dropped, and with it the listening sockets, which remove their socket
    // files when dropped.
    event_loop: UpgradeLoop<Data>,
    // The token of this source in the event loop, to enable it again after a pause.
    token: Rc<Cell<Option<RegistrationToken>>>,
}

/// Inserts the event source of a listening socket so that accept() errors don't end the event
/// loop.
pub fn insert_listening_socket<S, F, Data: 'static>(
    event_loop: &LoopHandle<'static, Data>,
    name: String,
    on_error: OnAcceptError,
    source: S,
    callback: F,
) -> Result<RegistrationToken, InsertError<ListeningSocket<S, Data>>>
where
    S: EventSource<Error = io::Error> + 'static,
    F: FnMut(S::Event, &mut S::Metadata, &mut Data) -> S::Ret + 'static,
{
    let token = Rc::new(Cell::new(None));
    let weak = event_loop.downgrade();
    let source = ListeningSocket {
        inner: source,
        name,
        on_error,
        event_loop: Rc::new(move || weak.upgrade()),
        token: token.clone(),
    };
    let registration = event_loop.insert_source(source, callback)?;
    token.set(Some(registration));
    Ok(registration)
}

impl<S, Data: 'static> ListeningSocket<S, Data> {
    fn handle_error(&self, err: io::Error) -> PostAction {
        let action = action_for_error(&err, self.on_error);
        match action {
            PostAction::Continue => {
                debug!(
                    "transient error accepting a connection on {}: {err:?}",
                    self.name
                );
            }
            PostAction::Remove => {
                warn!(
                    "error accepting a connection on {}, closing the socket: {err:?}",
                    self.name
                );
            }
            _ => {
                warn!(
                    "error accepting a connection on {}, pausing for {ACCEPT_ERROR_PAUSE:?}: \
                     {err:?}",
                    self.name
                );

                let Some(token) = self.token.get() else {
                    return PostAction::Continue;
                };
                let Some(event_loop) = (self.event_loop)() else {
                    return PostAction::Continue;
                };

                let upgrade = self.event_loop.clone();
                let name = self.name.clone();
                let timer = Timer::from_duration(ACCEPT_ERROR_PAUSE);
                let res = event_loop.insert_source(timer, move |_, _, _| {
                    let Some(event_loop) = upgrade() else {
                        return TimeoutAction::Drop;
                    };
                    match event_loop.enable(&token) {
                        Ok(()) => TimeoutAction::Drop,
                        // The source was removed in the meantime.
                        Err(calloop::Error::InvalidToken) => TimeoutAction::Drop,
                        // Registering with epoll again can fail for lack of resources too.
                        Err(err) => {
                            warn!(
                                "error accepting connections on {name} again, retrying in \
                                 {ACCEPT_ERROR_PAUSE:?}: {err:?}"
                            );
                            TimeoutAction::ToDuration(ACCEPT_ERROR_PAUSE)
                        }
                    }
                });
                if let Err(err) = res {
                    warn!("error inserting the timer to accept connections again: {err:?}");
                    return PostAction::Continue;
                }
            }
        }

        action
    }
}

/// Picks what to do with a listening socket source after accept() failed with `err`.
fn action_for_error(err: &io::Error, on_error: OnAcceptError) -> PostAction {
    // A signal interrupted accept(), or a connection was aborted before it was accepted: the
    // next poll can accept the other pending connections.
    if matches!(
        err.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
    ) {
        return PostAction::Continue;
    }

    match on_error {
        OnAcceptError::Pause => PostAction::Disable,
        OnAcceptError::Remove => PostAction::Remove,
    }
}

impl<S: EventSource<Error = io::Error>, Data: 'static> EventSource for ListeningSocket<S, Data> {
    type Event = S::Event;
    type Metadata = S::Metadata;
    type Ret = S::Ret;
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        callback: F,
    ) -> io::Result<PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        match self.inner.process_events(readiness, token, callback) {
            Ok(action) => Ok(action),
            Err(err) => Ok(self.handle_error(err)),
        }
    }

    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.register(poll, token_factory)
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.inner.reregister(poll, token_factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.inner.unregister(poll)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use calloop::generic::Generic;
    use calloop::{EventLoop, Interest, Mode};

    use super::*;

    #[test]
    fn source_does_not_keep_the_event_loop_alive() {
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let source = Generic::new(socket, Interest::READ, Mode::Level);

        let alive = Rc::new(());
        let marker = alive.clone();
        insert_listening_socket(
            &event_loop.handle(),
            String::from("test"),
            OnAcceptError::Pause,
            source,
            move |_, _, _| {
                let _ = &marker;
                Ok(PostAction::Continue)
            },
        )
        .unwrap();
        assert_eq!(Rc::strong_count(&alive), 2);

        // Dropping the loop drops its sources, and the source's callback with them.
        drop(event_loop);
        assert_eq!(Rc::strong_count(&alive), 1);
    }

    fn os_error(code: i32) -> io::Error {
        io::Error::from_raw_os_error(code)
    }

    #[test]
    fn accept_error_actions() {
        use OnAcceptError::*;

        for on_error in [Pause, Remove] {
            // Transient errors: retry on the next poll.
            assert_eq!(
                action_for_error(&os_error(libc::EINTR), on_error),
                PostAction::Continue
            );
            assert_eq!(
                action_for_error(&os_error(libc::ECONNABORTED), on_error),
                PostAction::Continue
            );
        }

        // Out of resources, or a socket that isn't listening.
        for code in [
            libc::EMFILE,
            libc::ENFILE,
            libc::ENOBUFS,
            libc::ENOMEM,
            libc::EINVAL,
        ] {
            assert_eq!(
                action_for_error(&os_error(code), Pause),
                PostAction::Disable
            );
            assert_eq!(
                action_for_error(&os_error(code), Remove),
                PostAction::Remove
            );
        }
    }
}
