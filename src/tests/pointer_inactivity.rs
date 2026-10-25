use std::thread::sleep;
use std::time::Duration;

use niri_config::Config;

use super::*;

const TIMEOUT: Duration = Duration::from_millis(100);

fn set_up(timeout_ms: u64) -> Fixture {
    let config = format!("cursor {{ hide-after-inactive-ms {timeout_ms}; }}");
    let mut f = Fixture::with_config(Config::parse_mem(&config).unwrap());
    f.add_output(1, (1920, 1080));
    // Startup already counted as activity this iteration.
    f.state.server.dispatch();
    f
}

/// Pointer activity, then an event loop iteration, which runs the timer if it's due.
///
/// The timer only ever runs right after activity here, so a slow machine can't make the pointer
/// look inactive.
fn activity(f: &mut Fixture) {
    f.niri().reset_pointer_inactivity_timer();
    f.state.server.dispatch();
}

/// An event loop iteration after `duration` without activity.
fn idle(f: &mut Fixture, duration: Duration) {
    sleep(duration);
    f.state.server.dispatch();
}

#[test]
fn pointer_hides_only_after_timeout_without_activity() {
    let mut f = set_up(TIMEOUT.as_millis() as u64);
    activity(&mut f);
    let timer = f.niri().pointer_inactivity_timer;
    assert!(timer.is_some());

    // Activity more often than the timeout keeps the pointer visible for several timeouts, with
    // the same timer.
    for _ in 0..12 {
        sleep(TIMEOUT / 4);
        activity(&mut f);
        assert!(f.niri().pointer_visibility.is_visible());
        assert_eq!(f.niri().pointer_inactivity_timer, timer);
    }

    idle(&mut f, TIMEOUT * 3 / 2);
    assert!(!f.niri().pointer_visibility.is_visible());
}

#[test]
fn pointer_inactivity_timeout_follows_config_reload() {
    // Long enough not to expire during the test.
    let mut f = set_up(60_000);
    activity(&mut f);

    let config = format!(
        "cursor {{ hide-after-inactive-ms {}; }}",
        TIMEOUT.as_millis()
    );
    f.niri_state()
        .reload_config(Ok(Config::parse_mem(&config).unwrap()));
    idle(&mut f, TIMEOUT * 3 / 2);
    assert!(!f.niri().pointer_visibility.is_visible());
}
