use smithay::wayland::idle_inhibit::IdleInhibitHandler as _;

use super::*;

#[test]
fn surface_stays_inhibited_until_its_last_inhibitor_is_destroyed() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    let surface = mapped.toplevel().wl_surface().clone();

    // smithay calls inhibit() for every zwp_idle_inhibitor_v1 created on the surface, and
    // uninhibit() for every one destroyed.
    let state = f.niri_state();
    state.inhibit(surface.clone());
    state.inhibit(surface.clone());

    state.uninhibit(surface.clone());
    assert!(state.niri.idle_inhibiting_surfaces.contains_key(&surface));

    state.uninhibit(surface.clone());
    assert!(!state.niri.idle_inhibiting_surfaces.contains_key(&surface));
}
