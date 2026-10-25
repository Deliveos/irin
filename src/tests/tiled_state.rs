use client::ClientId;
use insta::assert_snapshot;
use niri_config::Config;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

fn set_up(config: &str) -> (Fixture, ClientId, WlSurface) {
    let config = Config::parse_mem(config).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let surface = map_window(&mut f, id);

    (f, id, surface)
}

fn map_window(f: &mut Fixture, id: ClientId) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let _ = f.client(id).window(&surface).recent_configures();

    surface
}

const VIM_RULE: &str = r##"
window-rule {
    match title="^vim"
    tiled-state true
}
"##;

#[test]
fn title_change_applies_tiled_state_rule() {
    let (mut f, id, surface) = set_up(VIM_RULE);

    f.client(id).window(&surface).set_title("vim");
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, TiledLeft, TiledRight, TiledTop, TiledBottom]"
    );
}

#[test]
fn title_change_before_map_applies_tiled_state_rule() {
    let config = Config::parse_mem(VIM_RULE).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: []"
    );

    // Matches the rule after the initial configure, but before mapping.
    window.set_title("vim");
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [TiledLeft, TiledRight, TiledTop, TiledBottom, Activated]"
    );
}

#[test]
fn config_reload_before_map_applies_prefer_no_csd() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: []"
    );

    // Changes after the initial configure, but before mapping.
    let config = Config::parse_mem("prefer-no-csd").unwrap();
    f.niri_state().reload_config(Ok(config));

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [TiledLeft, TiledRight, TiledTop, TiledBottom, Activated]"
    );
}

#[test]
fn focus_change_applies_tiled_state_rule() {
    let config = r##"
window-rule {
    match is-focused=false
    tiled-state true
}
"##;
    let (mut f, id, first) = set_up(config);
    let second = map_window(&mut f, id);

    // Opening the second window unfocused the first one.
    assert_snapshot!(
        f.client(id).window(&first).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [TiledLeft, TiledRight, TiledTop, TiledBottom]"
    );

    f.niri().layout.focus_left();
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&first).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );
    assert_snapshot!(
        f.client(id).window(&second).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [TiledLeft, TiledRight, TiledTop, TiledBottom]"
    );
}

#[test]
fn config_reload_applies_tiled_state_rule() {
    let (mut f, id, surface) = set_up("");

    let config = Config::parse_mem("window-rule { tiled-state true; }").unwrap();
    f.niri_state().reload_config(Ok(config));
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, TiledLeft, TiledRight, TiledTop, TiledBottom]"
    );
}

#[test]
fn config_reload_applies_prefer_no_csd() {
    let (mut f, id, surface) = set_up("");

    let config = Config::parse_mem("prefer-no-csd").unwrap();
    f.niri_state().reload_config(Ok(config));
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, TiledLeft, TiledRight, TiledTop, TiledBottom]"
    );
}

#[test]
fn end_of_startup_reverts_at_startup_tiled_state_rule() {
    let config = r##"
window-rule {
    match at-startup=true
    tiled-state true
}
"##;
    let (mut f, id, surface) = set_up(config);

    // What the startup timer does.
    f.niri().is_at_startup = false;
    f.niri().recompute_window_rules();
    f.double_roundtrip(id);

    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );
}
