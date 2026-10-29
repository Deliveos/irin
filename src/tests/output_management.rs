use niri_config::{Config, FloatOrInt, OutputName, Outputs};

use super::client::ClientId;
use super::*;
use crate::protocols::output_management::OutputManagementHandler as _;

#[test]
fn applied_config_wins_over_other_matching_entries() {
    // Both entries match headless-1: by connector, and by make, model and serial.
    let config = Config::parse_mem(
        r#"
        output "headless-1" { scale 1; }
        output "niri headless 1" { scale 1; }
        "#,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    // wlr-output-management applies a changed copy of the first matching entry.
    let mut output = f.niri().config.borrow().outputs.0[0].clone();
    output.scale = Some(FloatOrInt(2.));
    f.niri_state().apply_output_config(Outputs(vec![output]));

    let scale = f.niri_output(1).current_scale().fractional_scale();
    assert_eq!(scale, 2.);
}

#[test]
fn applied_config_does_not_shadow_entries_for_other_monitors() {
    // The first entry is for another monitor, which can later be connected to headless-1.
    let config = Config::parse_mem(
        r#"
        output "niri headless 2" { scale 3; }
        output "headless-1" { scale 1; }
        "#,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    let mut output = f.niri().config.borrow().outputs.0[1].clone();
    output.scale = Some(FloatOrInt(2.));
    f.niri_state().apply_output_config(Outputs(vec![output]));
    let scale = f.niri_output(1).current_scale().fractional_scale();
    assert_eq!(scale, 2.);

    // The other monitor still gets its own entry when connected to headless-1.
    let other = OutputName {
        connector: String::from("headless-1"),
        make: Some(String::from("niri")),
        model: Some(String::from("headless")),
        serial: Some(String::from("2")),
    };
    let config = f.niri().config.borrow();
    let scale = config.outputs.find(&other).and_then(|o| o.scale);
    assert_eq!(scale, Some(FloatOrInt(3.)));
}

fn mode(width: u16, height: u16, refresh_rate: u32) -> niri_ipc::Mode {
    niri_ipc::Mode {
        width,
        height,
        refresh_rate,
        is_preferred: false,
    }
}

/// Changes the backend's IPC state for the only output, as a mode switch on a TTY does, and sends
/// it to wlr-output-management clients.
fn set_ipc_modes(f: &mut Fixture, modes: Vec<niri_ipc::Mode>, current_mode: Option<usize>) {
    let ipc_outputs = f.niri_state().backend.ipc_outputs();
    {
        let mut ipc_outputs = ipc_outputs.lock().unwrap();
        let output = ipc_outputs.values_mut().next().unwrap();
        output.modes = modes;
        output.current_mode = current_mode;
    }
    f.niri().ipc_outputs_changed = true;
    f.niri_state().refresh_ipc_outputs();
}

/// Returns whether the client's head is enabled, and the width, height and refresh rate of its
/// current mode.
fn client_current_mode(f: &mut Fixture, id: ClientId) -> (bool, Option<(i32, i32, i32)>) {
    let state = &f.client(id).state;
    let head = state
        .output_heads
        .iter()
        .find(|head| head.name.as_deref() == Some("headless-1"))
        .unwrap();
    let mode = head.current_mode.as_ref().map(|mode| {
        let mode = &state.output_modes[mode];
        (mode.size.0, mode.size.1, mode.refresh)
    });
    (head.enabled, mode)
}

#[test]
fn current_mode_is_sent_when_a_custom_mode_is_added() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(
        &mut f,
        vec![mode(1920, 1080, 60_000), mode(1920, 1080, 144_000)],
        Some(1),
    );

    let id = f.add_client();
    f.roundtrip(id);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (true, Some((1920, 1080, 144_000))));

    // Switching to a custom mode inserts it at index 0, so the other modes move one place over.
    set_ipc_modes(
        &mut f,
        vec![
            mode(1280, 720, 50_000),
            mode(1920, 1080, 60_000),
            mode(1920, 1080, 144_000),
        ],
        Some(0),
    );
    f.roundtrip(id);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (true, Some((1280, 720, 50_000))));

    // And switching back removes it again.
    set_ipc_modes(
        &mut f,
        vec![mode(1920, 1080, 60_000), mode(1920, 1080, 144_000)],
        Some(1),
    );
    f.roundtrip(id);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (true, Some((1920, 1080, 144_000))));
}

#[test]
fn enabling_an_output_with_a_custom_mode_is_sent() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(&mut f, vec![mode(1920, 1080, 60_000)], None);

    let id = f.add_client();
    f.roundtrip(id);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (false, None));

    // Turning the output on with a custom mode adds that mode to the list.
    set_ipc_modes(
        &mut f,
        vec![mode(1280, 720, 50_000), mode(1920, 1080, 60_000)],
        Some(0),
    );
    f.roundtrip(id);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (true, Some((1280, 720, 50_000))));
}
