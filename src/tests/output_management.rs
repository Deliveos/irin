use niri_config::{Config, FloatOrInt, OutputName, Outputs};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::ZwlrOutputManagerV1;

use super::client::{ClientId, OutputConfigurationResult, OutputHead};
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

/// Returns the width, height, refresh rate and preferred flag of the client's modes, sorted.
fn client_modes(f: &mut Fixture, id: ClientId) -> Vec<(i32, i32, i32, bool)> {
    let state = &f.client(id).state;
    let mut modes: Vec<_> = state
        .output_modes
        .values()
        .map(|mode| (mode.size.0, mode.size.1, mode.refresh, mode.preferred))
        .collect();
    modes.sort();
    modes
}

#[test]
fn preferred_mode_stays_preferred_when_a_custom_mode_is_added() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let preferred = niri_ipc::Mode {
        is_preferred: true,
        ..mode(1920, 1080, 60_000)
    };
    let modes = vec![preferred, mode(1920, 1080, 144_000)];
    set_ipc_modes(&mut f, modes.clone(), Some(1));

    let id = f.add_client();
    f.roundtrip(id);
    let expected = [(1920, 1080, 60_000, true), (1920, 1080, 144_000, false)];
    assert_eq!(client_modes(&mut f, id), expected);

    // Switching to a custom mode inserts it at index 0, so the other modes move one place over.
    let custom = vec![
        mode(1280, 720, 50_000),
        preferred,
        mode(1920, 1080, 144_000),
    ];
    set_ipc_modes(&mut f, custom, Some(0));
    f.roundtrip(id);
    let with_custom = [
        (1280, 720, 50_000, false),
        (1920, 1080, 60_000, true),
        (1920, 1080, 144_000, false),
    ];
    assert_eq!(client_modes(&mut f, id), with_custom);
    let current = client_current_mode(&mut f, id);
    assert_eq!(current, (true, Some((1280, 720, 50_000))));

    // And switching back removes it again.
    set_ipc_modes(&mut f, modes, Some(1));
    f.roundtrip(id);
    assert_eq!(client_modes(&mut f, id), expected);
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

/// Returns the client's head for headless-1 that this manager announced.
fn head_of<'a>(f: &'a mut Fixture, id: ClientId, manager: &ZwlrOutputManagerV1) -> &'a OutputHead {
    f.client(id)
        .state
        .output_heads
        .iter()
        .find(|head| head.manager == *manager && head.name.as_deref() == Some("headless-1"))
        .unwrap()
}

/// Returns the width, height and refresh rate of the current mode of the manager's head.
fn current_mode_of(
    f: &mut Fixture,
    id: ClientId,
    manager: &ZwlrOutputManagerV1,
) -> Option<(i32, i32, i32)> {
    let mode = head_of(f, id, manager).current_mode.clone()?;
    let mode = &f.client(id).state.output_modes[&mode];
    Some((mode.size.0, mode.size.1, mode.refresh))
}

/// Adds a client whose two parts each bind wlr-output-management, and returns both managers.
fn add_client_with_two_managers(
    f: &mut Fixture,
) -> (ClientId, ZwlrOutputManagerV1, ZwlrOutputManagerV1) {
    let id = f.add_client();
    f.roundtrip(id);
    let first = f.client(id).state.output_manager.clone().unwrap();
    let second = f.client(id).bind_output_manager();
    f.roundtrip(id);
    (id, first, second)
}

fn two_modes() -> Vec<niri_ipc::Mode> {
    vec![mode(1920, 1080, 60_000), mode(1920, 1080, 144_000)]
}

#[test]
fn output_manager_bound_twice_gets_changes_on_both() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(&mut f, two_modes(), Some(0));
    let (id, first, second) = add_client_with_two_managers(&mut f);

    set_ipc_modes(&mut f, two_modes(), Some(1));
    f.roundtrip(id);
    for manager in [&first, &second] {
        let current = current_mode_of(&mut f, id, manager);
        assert_eq!(current, Some((1920, 1080, 144_000)));
    }
}

#[test]
fn releasing_a_head_of_one_output_manager_keeps_the_other() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(&mut f, two_modes(), Some(0));
    let (id, first, second) = add_client_with_two_managers(&mut f);

    head_of(&mut f, id, &first).head.release();
    f.roundtrip(id);

    set_ipc_modes(&mut f, two_modes(), Some(1));
    f.roundtrip(id);
    let current = current_mode_of(&mut f, id, &second);
    assert_eq!(current, Some((1920, 1080, 144_000)));
}

#[test]
fn stopping_one_output_manager_keeps_the_other() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(&mut f, two_modes(), Some(0));
    let (id, first, second) = add_client_with_two_managers(&mut f);

    first.stop();
    f.roundtrip(id);

    set_ipc_modes(&mut f, two_modes(), Some(1));
    f.roundtrip(id);
    let current = current_mode_of(&mut f, id, &second);
    assert_eq!(current, Some((1920, 1080, 144_000)));
}

#[test]
fn set_mode_works_through_the_first_of_two_output_managers() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    set_ipc_modes(&mut f, two_modes(), Some(0));
    let (id, first, _second) = add_client_with_two_managers(&mut f);

    // Pick the first manager's 144 Hz mode.
    let head = head_of(&mut f, id, &first).head.clone();
    let modes = head_of(&mut f, id, &first).modes.clone();
    let state = &f.client(id).state;
    let mode = modes
        .into_iter()
        .find(|m| {
            state
                .output_modes
                .get(m)
                .is_some_and(|m| m.refresh == 144_000)
        })
        .unwrap();

    let client = f.client(id);
    let qh = client.qh.clone();
    let conf = first.create_configuration(client.state.output_manager_serial, &qh, ());
    let conf_head = conf.enable_head(&head, &qh, ());
    conf_head.set_mode(&mode);
    conf.apply();
    f.roundtrip(id);

    let result = f.client(id).state.output_configurations.get(&conf).copied();
    assert_eq!(result, Some(OutputConfigurationResult::Succeeded));
}
