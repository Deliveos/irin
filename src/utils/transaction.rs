use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use atomic::Ordering;
use calloop::ping::{make_ping, Ping};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::wayland_server::Client;
use smithay::wayland::compositor::{Blocker, BlockerState};

/// Default time limit, after which the transaction completes.
///
/// Serves to avoid hanging when a client fails to respond to a configure promptly.
const TIME_LIMIT: Duration = Duration::from_millis(300);

/// Transaction between Wayland clients.
///
/// How to use it:
/// 1. Create a transaction with [`Transaction::new()`].
/// 2. Clone it as many times as you need.
/// 3. Before adding the transaction as a commit blocker, remember to call
///    [`Transaction::add_notification()`] to receive a notification when the transaction completes.
/// 4. Before adding the transaction as a commit blocker, remember to call
///    [`Transaction::register_deadline_timer()`] to make sure the transaction completes when
///    reaching the deadline.
/// 5. In your surface pre-commit handler, if the transaction corresponding to that commit isn't
///    ready, get a blocker with [`Transaction::blocker()`] and add it to the surface.
#[derive(Debug, Clone)]
pub struct Transaction {
    inner: Arc<Inner>,
    deadline: Rc<RefCell<Deadline>>,
}

/// Blocker for a [`Transaction`].
#[derive(Debug)]
pub struct TransactionBlocker(Weak<Inner>);

#[derive(Debug)]
enum Deadline {
    NotRegistered(Instant),
    /// The deadline timer is registered.
    ///
    /// Pinging `remove` removes the timer. Without it (when creating or inserting the ping
    /// failed), the timer stays until the deadline; if the transaction is gone by then, it does
    /// nothing.
    Registered {
        remove: Option<Ping>,
    },
}

#[derive(Debug)]
struct Inner {
    /// Whether the transaction is completed.
    completed: AtomicBool,
    /// Notifications to send out upon completing the transaction.
    notifications: Mutex<Option<(Sender<Client>, Vec<Client>)>>,
}

impl Transaction {
    /// Creates a new transaction.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::new()),
            deadline: Rc::new(RefCell::new(Deadline::NotRegistered(
                Instant::now() + TIME_LIMIT,
            ))),
        }
    }

    /// Gets a blocker for this transaction.
    pub fn blocker(&self) -> TransactionBlocker {
        trace!(transaction = ?Arc::as_ptr(&self.inner), "generating blocker");
        TransactionBlocker(Arc::downgrade(&self.inner))
    }

    /// Adds a notification for when this transaction completes.
    pub fn add_notification(&self, sender: Sender<Client>, client: Client) {
        if self.is_completed() {
            error!("tried to add notification to a completed transaction");
            return;
        }

        let mut guard = self.inner.notifications.lock().unwrap();
        guard.get_or_insert((sender, Vec::new())).1.push(client);
    }

    /// Registers this transaction's deadline timer on an event loop.
    pub fn register_deadline_timer<T: 'static>(&self, event_loop: &LoopHandle<'static, T>) {
        let mut cell = self.deadline.borrow_mut();
        if let Deadline::NotRegistered(deadline) = *cell {
            let timer = Timer::from_deadline(deadline);
            let inner = Arc::downgrade(&self.inner);
            let token = event_loop
                .insert_source(timer, move |_, _, _| {
                    let _span = trace_span!("deadline timer", transaction = ?Weak::as_ptr(&inner))
                        .entered();

                    // FIXME: come up with some way to control the deadline timer from tests.
                    #[cfg(not(test))]
                    if let Some(inner) = inner.upgrade() {
                        trace!("deadline reached, completing transaction");
                        inner.complete();
                    } else {
                        // We should remove the timer automatically. But this callback can still
                        // just happen to run while the ping callback is scheduled, or there may
                        // be no ping, leading to this branch being legitimately taken.
                        trace!("transaction completed without removing the timer");
                    }

                    TimeoutAction::Drop
                })
                .inspect_err(|err| {
                    error!("error inserting the transaction deadline timer: {err:?}")
                });
            let Ok(token) = token else {
                // Without the timer, a client that never commits would hold up the transaction
                // forever, so complete it right away.
                self.inner.complete();
                return;
            };

            *cell = Deadline::Registered {
                remove: insert_removal_ping(event_loop, token),
            };
        }
    }

    /// Returns whether this transaction has already completed.
    pub fn is_completed(&self) -> bool {
        self.inner.is_completed()
    }

    /// Returns whether this is the last instance of this transaction.
    pub fn is_last(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }
}

/// Adds a ping source that removes the deadline timer with `token`.
///
/// Returns `None` if that fails, for example when out of file descriptors.
fn insert_removal_ping<T: 'static>(
    event_loop: &LoopHandle<'static, T>,
    token: RegistrationToken,
) -> Option<Ping> {
    let (ping, source) = match make_ping() {
        Ok(x) => x,
        Err(err) => {
            warn!("error creating a ping to remove the transaction deadline timer: {err:?}");
            return None;
        }
    };

    let loop_handle = event_loop.clone();
    let res = event_loop.insert_source(source, move |_, _, _| {
        loop_handle.remove(token);
    });
    if let Err(err) = res {
        warn!("error inserting the ping to remove the transaction deadline timer: {err:?}");
        return None;
    }

    Some(ping)
}

impl Drop for Transaction {
    fn drop(&mut self) {
        let _span = trace_span!("drop", transaction = ?Arc::as_ptr(&self.inner)).entered();

        if self.is_last() {
            // If this was the last transaction, complete it.
            trace!("last transaction dropped, completing");
            self.inner.complete();

            // Also remove the timer.
            if let Deadline::Registered {
                remove: Some(remove),
            } = &*self.deadline.borrow()
            {
                remove.ping();
            };
        }
    }
}

impl TransactionBlocker {
    pub fn completed() -> Self {
        Self(Weak::new())
    }
}

impl Blocker for TransactionBlocker {
    fn state(&self) -> BlockerState {
        if self.0.upgrade().is_none_or(|x| x.is_completed()) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

impl Inner {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            notifications: Mutex::new(None),
        }
    }

    fn is_completed(&self) -> bool {
        self.completed.load(Ordering::Relaxed)
    }

    fn complete(&self) {
        self.completed.store(true, Ordering::Relaxed);

        let mut guard = self.notifications.lock().unwrap();
        if let Some((sender, clients)) = guard.take() {
            for client in clients {
                if let Err(err) = sender.send(client) {
                    warn!("error sending blocker notification: {err:?}");
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::AsRawFd as _;
    use std::process::Command;

    use calloop::EventLoop;

    use super::*;

    /// Lowers the soft open file limit so that creating a file descriptor fails with `EMFILE`.
    fn use_up_file_descriptors() {
        // The lowest free file descriptor; every one below it is open.
        let lowest_free = File::open("/dev/null").unwrap().as_raw_fd();

        let mut rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) },
            0
        );
        rlim.rlim_cur = lowest_free as libc::rlim_t;
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &rlim) }, 0);
    }

    #[test]
    fn deadline_timer_without_file_descriptors() {
        // The open file limit is per process, so run this test alone in a child process.
        const IN_CHILD: &str = "NORI_TEST_TRANSACTION_IN_CHILD";
        if std::env::var_os(IN_CHILD).is_none() {
            let name = "utils::transaction::tests::deadline_timer_without_file_descriptors";
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--nocapture"])
                .env(IN_CHILD, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stdout}\n{stderr}");
            // Make sure that the child ran this test rather than nothing.
            assert!(stdout.contains("1 passed"), "{stdout}\n{stderr}");
            return;
        }

        let event_loop = EventLoop::<()>::try_new().unwrap();
        let transaction = Transaction::new();

        use_up_file_descriptors();
        assert!(make_ping().is_err());
        assert!(
            crate::utils::insert_channel(&event_loop.handle(), |_: (), _: &mut ()| ()).is_err()
        );

        transaction.register_deadline_timer(&event_loop.handle());
        assert!(matches!(
            *transaction.deadline.borrow(),
            Deadline::Registered { remove: None }
        ));
    }
}
