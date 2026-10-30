use std::fmt::{self, Write as _};

use insta::assert_snapshot;
use niri_config::Config;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use super::*;
use crate::layout::LayoutElement as _;
use crate::utils::spawning::store_and_increase_nofile_rlimit;
use crate::utils::with_toplevel_role;

#[test]
fn simple_no_workspaces() {
    let mut f = Fixture::new();

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 0 × 0, bounds: 0 × 0, states: []"
    );

    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 100 × 688, bounds: 1248 × 688, states: []"
    );
}

#[test]
fn simple() {
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

    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );
}

#[test]
#[should_panic(expected = "Protocol error 3 on object xdg_surface")]
fn dont_ack_initial_configure() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    // Don't ack the configure.
    window.commit();
    f.double_roundtrip(id);
}

#[derive(Clone, Copy)]
enum WantFullscreen {
    No,
    UnsetBeforeInitial,
    BeforeInitial(Option<&'static str>),
    UnsetAfterInitial,
    AfterInitial(Option<&'static str>),
}

impl fmt::Display for WantFullscreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WantFullscreen::No => write!(f, "U")?,
            WantFullscreen::UnsetBeforeInitial => write!(f, "BU")?,
            WantFullscreen::UnsetAfterInitial => write!(f, "AU")?,
            WantFullscreen::BeforeInitial(m) => write!(f, "B{}", m.unwrap_or("N"))?,
            WantFullscreen::AfterInitial(m) => write!(f, "A{}", m.unwrap_or("N"))?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum WantMaximized {
    No,
    UnsetBeforeInitial,
    BeforeInitial,
    UnsetAfterInitial,
    AfterInitial,
}

impl fmt::Display for WantMaximized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WantMaximized::No => write!(f, "U")?,
            WantMaximized::UnsetBeforeInitial => write!(f, "BU")?,
            WantMaximized::UnsetAfterInitial => write!(f, "AU")?,
            WantMaximized::BeforeInitial => write!(f, "B")?,
            WantMaximized::AfterInitial => write!(f, "A")?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SetParent {
    BeforeInitial(&'static str),
    AfterInitial(&'static str),
}

impl fmt::Display for SetParent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetParent::BeforeInitial(m) => write!(f, "B{m}")?,
            SetParent::AfterInitial(m) => write!(f, "A{m}")?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DefaultSize {
    WindowChooses,
    Proportion(&'static str),
    Fixed(&'static str),
}

impl fmt::Display for DefaultSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefaultSize::WindowChooses => write!(f, "U"),
            DefaultSize::Proportion(prop) => write!(f, "P{prop}"),
            DefaultSize::Fixed(fixed) => write!(f, "F{fixed}"),
        }
    }
}

#[test]
fn target_output_and_workspaces() {
    store_and_increase_nofile_rlimit();

    // Here we test a massive powerset of settings that can affect where a window opens:
    //
    // * open-on-workspace
    // * open-on-output
    // * has parent (windows will open next to their parent)
    // * want fullscreen (windows can request the target fullscreen output)
    // * open-fullscreen (can deny the fullscreen request)

    let open_on_workspace = [None, Some("1"), Some("2")];
    let open_on_output = [None, Some("1"), Some("2")];
    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::BeforeInitial(Some("1")),
        WantFullscreen::BeforeInitial(Some("2")),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
        WantFullscreen::AfterInitial(Some("1")),
        WantFullscreen::AfterInitial(Some("2")),
    ];
    let set_parent = [
        None,
        Some(SetParent::BeforeInitial("1")),
        Some(SetParent::BeforeInitial("2")),
        Some(SetParent::AfterInitial("1")),
        Some(SetParent::AfterInitial("2")),
    ];

    let mut powerset = Vec::new();
    for ws in open_on_workspace {
        for out in open_on_output {
            for fs in open_fullscreen {
                for wfs in want_fullscreen {
                    for sp in set_parent {
                        powerset.push((ws, out, fs, wfs, sp));
                    }
                }
            }
        }
    }

    powerset.into_par_iter().for_each(|(ws, out, fs, wfs, sp)| {
        check_target_output_and_workspace(ws, out, fs, wfs, sp);
    });
}

fn check_target_output_and_workspace(
    open_on_workspace: Option<&str>,
    open_on_output: Option<&str>,
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    set_parent: Option<SetParent>,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
output "headless-2" {
    layout {
        border {
            on
        }
    }
}

workspace "ws-1" {
    open-on-output "headless-1"
}

workspace "ws-2" {
    open-on-output "headless-2"

    layout {
        border {
            width 10
        }

        default-column-width {
            fixed 500
        }
    }
}

window-rule {
    exclude title="parent"

"##,
    );

    if let Some(x) = open_on_workspace {
        writeln!(config, "    open-on-workspace \"ws-{x}\"").unwrap();
        snapshot_suffix.push(format!("ws{x}"));
    }

    if let Some(x) = open_on_output {
        writeln!(config, "    open-on-output \"headless-{x}\"").unwrap();
        snapshot_suffix.push(format!("out{x}"));
    }

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }
    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    if let Some(set_parent) = set_parent {
        let mon = match set_parent {
            SetParent::BeforeInitial(mon) => mon,
            SetParent::AfterInitial(mon) => mon,
        };
        write!(
            config,
            "

window-rule {{
    match title=\"parent\"
    open-on-output \"headless-{mon}\"
}}"
        )
        .unwrap();

        snapshot_desc.push(format!("set parent: {set_parent}"));
        snapshot_suffix.push(format!("sp{set_parent}"));
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let mut parent = None;
    if set_parent.is_some() {
        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        parent = Some(window.xdg_toplevel.clone());
        window.set_title("parent");
        window.commit();
        f.roundtrip(id);

        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.ack_last_and_commit();
        f.roundtrip(id);
    }

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let Some(SetParent::BeforeInitial(_)) = set_parent {
        client.window(&surface).set_parent(parent.as_ref());
    }

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let Some(SetParent::AfterInitial(_)) = set_parent {
        client.window(&surface).set_parent(parent.as_ref());
    }

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let niri = f.niri();
    let (mon, ws_idx, ws, mapped) = niri
        .layout
        .workspaces()
        .find_map(|(mon, ws_idx, ws)| {
            ws.windows().find_map(|win| {
                if with_toplevel_role(win.toplevel(), |role| {
                    role.title.as_deref() != Some("parent")
                }) {
                    Some((mon, ws_idx, ws, win))
                } else {
                    None
                }
            })
        })
        .unwrap();
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    let mon = mon.unwrap().output_name().clone();
    let ws = ws.name().cloned().unwrap_or(String::from("unnamed"));

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
final monitor: {mon}
final workspace: {ws_idx} ({ws})

initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}

#[test]
fn target_size() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    store_and_increase_nofile_rlimit();

    // Here we test a massive powerset of settings that can affect the window size:
    //
    // * want fullscreen
    // * open-fullscreen
    // * open-maximized
    // * open-floating
    // * default-column-width
    // * border
    // * default-column-display normal, tabbed

    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
    ];
    let open_maximized = [None, Some("true")];
    let open_floating = [None, Some("true")];
    let default_column_width = [
        None,
        Some(DefaultSize::WindowChooses),
        Some(DefaultSize::Proportion("0.25")),
        Some(DefaultSize::Fixed("1000")),
    ];
    let default_window_height = [
        None,
        Some(DefaultSize::WindowChooses),
        Some(DefaultSize::Proportion("0.5")),
        Some(DefaultSize::Fixed("500")),
    ];
    let border = [false, true];
    let tabbed = [false, true];

    let mut powerset = Vec::new();
    for fs in open_fullscreen {
        for wfs in want_fullscreen {
            for om in open_maximized {
                for of in open_floating {
                    for dw in default_column_width {
                        for dh in default_window_height {
                            for b in border {
                                for t in tabbed {
                                    powerset.push((fs, wfs, om, of, dw, dh, b, t));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    powerset
        .into_par_iter()
        .for_each(|(fs, wfs, om, of, dw, dh, b, t)| {
            check_target_size(fs, wfs, om, of, dw, dh, b, t);
        });
}

#[allow(clippy::too_many_arguments)]
fn check_target_size(
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    open_maximized: Option<&str>,
    open_floating: Option<&str>,
    default_width: Option<DefaultSize>,
    default_height: Option<DefaultSize>,
    border: bool,
    tabbed: bool,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
window-rule {
"##,
    );

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }

    if let Some(x) = open_maximized {
        writeln!(config, "    open-maximized {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("om{x}"));
    }

    if let Some(x) = open_floating {
        writeln!(config, "    open-floating {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("of{x}"));
    }

    if let Some(x) = default_width {
        let value = match x {
            DefaultSize::WindowChooses => String::new(),
            DefaultSize::Proportion(prop) => format!("proportion {prop};"),
            DefaultSize::Fixed(fixed) => format!("fixed {fixed};"),
        };
        writeln!(config, "    default-column-width {{ {value} }}").unwrap();

        snapshot_suffix.push(format!("dw{x}"));
    }

    if let Some(x) = default_height {
        let value = match x {
            DefaultSize::WindowChooses => String::new(),
            DefaultSize::Proportion(prop) => format!("proportion {prop};"),
            DefaultSize::Fixed(fixed) => format!("fixed {fixed};"),
        };
        writeln!(config, "    default-window-height {{ {value} }}").unwrap();

        snapshot_suffix.push(format!("dh{x}"));
    }

    if border {
        writeln!(config, "    border {{ on; }}").unwrap();
        snapshot_suffix.push(String::from("b"));
    }

    if tabbed {
        writeln!(config, "    default-column-display \"tabbed\"").unwrap();
    }

    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    if tabbed {
        config.push_str(
            "\n
layout {
    tab-indicator {
        place-within-column
    }
}",
        );
        snapshot_suffix.push(String::from("t"));
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}

#[test]
fn fullscreen_maximize() {
    store_and_increase_nofile_rlimit();

    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
    ];
    let open_maximized = [None, Some("false"), Some("true")];
    let want_maximized = [
        WantMaximized::No,
        WantMaximized::UnsetBeforeInitial,
        WantMaximized::BeforeInitial,
        WantMaximized::UnsetAfterInitial,
        WantMaximized::AfterInitial,
    ];

    let mut powerset = Vec::new();
    for fs in open_fullscreen {
        for wfs in want_fullscreen {
            for tm in open_maximized {
                for wm in want_maximized {
                    powerset.push((fs, wfs, tm, wm));
                }
            }
        }
    }

    powerset.into_par_iter().for_each(|(fs, wfs, tm, wm)| {
        check_fullscreen_maximize(fs, wfs, tm, wm);
    });
}

fn check_fullscreen_maximize(
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    open_maximized: Option<&str>,
    want_maximized: WantMaximized,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
window-rule {
"##,
    );

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }

    if let Some(x) = open_maximized {
        writeln!(config, "    open-maximized-to-edges {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("tm{x}"));
    }

    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    match &want_maximized {
        WantMaximized::No => (),
        x => {
            snapshot_desc.push(format!("want maximized: {x}"));
            snapshot_suffix.push(format!("wm{x}"));
        }
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let WantMaximized::UnsetBeforeInitial = want_maximized {
        client.window(&surface).unset_maximized();
    } else if let WantMaximized::BeforeInitial = want_maximized {
        client.window(&surface).set_maximized();
    }

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let WantMaximized::UnsetAfterInitial = want_maximized {
        client.window(&surface).unset_maximized();
    } else if let WantMaximized::AfterInitial = want_maximized {
        client.window(&surface).set_maximized();
    }

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    // If the window ended up maximized, unmaximize it and output the configure.
    let mut post_unmaximize = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_maximized = mapped.sizing_mode().is_maximized();
    let win = mapped.window.clone();
    if is_maximized {
        f.niri().layout.set_maximized(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unmaximize = format!(
            "\n\nunmaximize configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}{post_unmaximize}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}

#[test]
fn maximize_and_fullscreen_after_initial_configure_use_target_workspace() {
    let config = Config::parse_mem(
        r##"
workspace "ws-2" {
    open-on-output "headless-2"
}

window-rule {
    open-on-workspace "ws-2"
}
"##,
    )
    .unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));

    // One window asks to be maximized and the other to be fullscreen, so that each request is
    // tested on its own.
    let id = f.add_client();
    let mut surfaces = Vec::new();
    for _ in 0..2 {
        let window = f.client(id).create_window();
        surfaces.push(window.surface.clone());
        window.commit();
    }
    f.roundtrip(id);

    // headless-2 isn't connected yet, so ws-2 is on headless-1.
    for surface in &surfaces {
        let window = f.client(id).window(surface);
        assert_eq!(
            window.format_recent_configures(),
            "size: 616 × 688, bounds: 1248 × 688, states: []"
        );
    }

    // Now ws-2 moves to headless-2, where the windows will map.
    f.add_output(2, (1920, 1080));
    f.roundtrip(id);

    let window = f.client(id).window(&surfaces[0]);
    window.set_maximized();
    f.roundtrip(id);
    let window = f.client(id).window(&surfaces[0]);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 1920 × 1080, bounds: 1888 × 1048, states: [Maximized]"
    );

    let window = f.client(id).window(&surfaces[1]);
    window.set_fullscreen(None);
    f.roundtrip(id);
    let window = f.client(id).window(&surfaces[1]);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 1920 × 1080, bounds: 1888 × 1048, states: [Fullscreen]"
    );

    for surface in &surfaces {
        let window = f.client(id).window(surface);
        window.attach_new_buffer();
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        // Already configured for headless-2, so it isn't resized when it maps.
        let window = f.client(id).window(surface);
        for configure in window.recent_configures() {
            assert_eq!(configure.size, (1920, 1080));
        }
    }

    let niri = f.niri();
    let (mon, _, ws) = niri
        .layout
        .workspaces()
        .find(|(_, _, ws)| ws.windows().next().is_some())
        .unwrap();
    assert_eq!(mon.unwrap().output_name(), "headless-2");
    assert_eq!(ws.name().map(String::as_str), Some("ws-2"));
    assert_eq!(ws.windows().count(), 2);
}

/// Maps a parent on headless-1 in ws-1, gives a dialog its initial configure while it follows its
/// parent, then moves the parent to headless-2. Then `request` sends a request for the dialog, and
/// the dialog maps. Checks that the dialog maps next to its parent on headless-2, and returns its
/// configures after the request.
fn check_request_before_map_for_dialog_that_follows_parent(
    request: impl FnOnce(&super::client::Window),
) -> String {
    let config = Config::parse_mem(
        r##"
workspace "ws-1"
"##,
    )
    .unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    // The parent opens on headless-1, in ws-1.
    let id = f.add_client();
    let parent = f.client(id).create_window();
    let parent_surface = parent.surface.clone();
    let parent_toplevel = parent.xdg_toplevel.clone();
    parent.commit();
    f.roundtrip(id);
    let parent = f.client(id).window(&parent_surface);
    parent.attach_new_buffer();
    parent.ack_last_and_commit();
    f.roundtrip(id);

    // The dialog gets its initial configure while following its parent, which is in ws-1.
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_parent(Some(&parent_toplevel));
    window.commit();
    f.roundtrip(id);
    let _ = f.client(id).window(&surface).format_recent_configures();

    // Then the parent moves to headless-2.
    let parent_window = f.niri().layout.windows().next().unwrap().1.window.clone();
    let output = f.niri_output(2);
    f.niri().layout.move_to_output(
        Some(&parent_window),
        &output,
        None,
        crate::layout::ActivateWindow::No,
    );

    request(f.client(id).window(&surface));
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    let configures = window.format_recent_configures();

    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let niri = f.niri();
    let monitors: Vec<_> = niri
        .layout
        .windows()
        .map(|(mon, _)| mon.unwrap().output_name().clone())
        .collect();
    assert_eq!(monitors, ["headless-2", "headless-2"]);

    configures
}

#[test]
fn maximize_before_map_keeps_a_dialog_next_to_its_parent() {
    // The dialog is sized for headless-2, where it will open next to its parent.
    let configures = check_request_before_map_for_dialog_that_follows_parent(|window| {
        window.set_maximized();
    });
    assert_eq!(
        configures,
        "size: 1920 × 1080, bounds: 1888 × 1048, states: [Maximized]"
    );
}

#[test]
fn unmaximize_before_map_keeps_a_dialog_next_to_its_parent() {
    // The dialog is sized for headless-2, where it will open next to its parent, rather than for
    // the output of the named workspace ws-1 that it was configured for.
    let configures = check_request_before_map_for_dialog_that_follows_parent(|window| {
        window.unset_maximized();
    });
    assert_eq!(configures, "size: 0 × 0, bounds: 1920 × 1080, states: []");
}

/// Maps a parent in ws-3 and gives a dialog its initial configure for ws-2, both on headless-1.
/// Then both workspaces move to headless-2, `request` sends a request for the dialog, and the
/// dialog maps. Checks that the dialog maps in ws-2 rather than next to its parent, as it would
/// without the request, and returns its configures after the request.
fn check_request_before_map_for_dialog_on_its_workspace(
    request: impl FnOnce(&super::client::Window),
) -> String {
    let config = Config::parse_mem(
        r##"
workspace "ws-2" {
    open-on-output "headless-2"
}

workspace "ws-3" {
    open-on-output "headless-2"
}

window-rule {
    match title="parent"
    open-on-workspace "ws-3"
}

window-rule {
    match title="dialog"
    open-on-workspace "ws-2"
}
"##,
    )
    .unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));

    // headless-2 isn't connected yet, so the parent opens in ws-3 on headless-1.
    let id = f.add_client();
    let parent = f.client(id).create_window();
    let parent_surface = parent.surface.clone();
    let parent_toplevel = parent.xdg_toplevel.clone();
    parent.set_title("parent");
    parent.commit();
    f.roundtrip(id);
    let parent = f.client(id).window(&parent_surface);
    parent.attach_new_buffer();
    parent.ack_last_and_commit();
    f.roundtrip(id);

    // The dialog gets its initial configure for ws-2, which is on headless-1 too.
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_title("dialog");
    window.set_parent(Some(&parent_toplevel));
    window.commit();
    f.roundtrip(id);
    let _ = f.client(id).window(&surface).format_recent_configures();

    // Now both workspaces move to headless-2, ws-3 with the parent.
    f.add_output(2, (1920, 1080));
    f.roundtrip(id);

    request(f.client(id).window(&surface));
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    let configures = window.format_recent_configures();

    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // The dialog opens in ws-2, not next to its parent in ws-3.
    let niri = f.niri();
    let windows: Vec<_> = niri
        .layout
        .workspaces()
        .map(|(mon, _, ws)| {
            (
                mon.map(|mon| mon.output_name().clone()),
                ws.name().cloned(),
                ws.windows().count(),
            )
        })
        .filter(|(_, _, count)| *count > 0)
        .collect();
    assert_eq!(
        windows,
        [
            (
                Some(String::from("headless-2")),
                Some(String::from("ws-2")),
                1
            ),
            (
                Some(String::from("headless-2")),
                Some(String::from("ws-3")),
                1
            ),
        ]
    );

    configures
}

#[test]
fn maximize_before_map_keeps_a_dialog_on_its_workspace() {
    let configures = check_request_before_map_for_dialog_on_its_workspace(|window| {
        window.set_maximized();
    });
    assert_eq!(
        configures,
        "size: 1920 × 1080, bounds: 1888 × 1048, states: [Maximized]"
    );
}

#[test]
fn unmaximize_before_map_keeps_a_dialog_on_its_workspace() {
    let configures = check_request_before_map_for_dialog_on_its_workspace(|window| {
        window.unset_maximized();
    });
    assert_eq!(configures, "size: 0 × 0, bounds: 1920 × 1080, states: []");
}
