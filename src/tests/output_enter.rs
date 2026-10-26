use client::ClientId;
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::Gravity;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

fn entered_outputs(f: &mut Fixture, id: ClientId, surface: &WlSurface) -> usize {
    f.client(id).state.surfaces[surface].entered_outputs.len()
}

#[test]
fn new_subsurfaces_and_popups_enter_the_output() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &surface), 1);

    // Let some event loop iterations pass with nothing changing.
    f.double_roundtrip(id);

    // A subsurface added after mapping, which appears with the parent's commit.
    let (subsurface, _) = f.client(id).state.create_subsurface(&surface);
    f.client(id).window(&surface).commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &subsurface), 1);

    f.double_roundtrip(id);

    // A desynchronized subsurface, which appears with its own commit.
    let (desync, desync_subsurface) = f.client(id).state.create_subsurface(&surface);
    desync_subsurface.set_desync();
    f.client(id).state.attach_pixel_buffer(&desync);
    desync.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &desync), 1);

    f.double_roundtrip(id);

    // A popup, which maps with its own commit.
    let xdg_surface = f.client(id).window(&surface).xdg_surface.clone();
    let (popup, _, _) = f
        .client(id)
        .state
        .create_popup(&xdg_surface, Gravity::BottomRight);
    f.roundtrip(id);
    f.client(id).state.attach_pixel_buffer(&popup);
    popup.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &popup), 1);

    f.double_roundtrip(id);

    // A popup entirely above and left of the window.
    let (popup, _, _) = f
        .client(id)
        .state
        .create_popup(&xdg_surface, Gravity::TopLeft);
    f.roundtrip(id);
    f.client(id).state.attach_pixel_buffer(&popup);
    popup.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_outputs(&mut f, id, &popup), 1);
}

fn entered_output_names(f: &mut Fixture, id: ClientId, surface: &WlSurface) -> Vec<String> {
    let state = &f.client(id).state;
    let entered = &state.surfaces[surface].entered_outputs;
    entered.iter().map(|o| state.outputs[o].clone()).collect()
}

#[test]
fn ime_popup_leaves_the_output_of_its_old_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let id = f.add_client();

    // A window on each output.
    for n in [1, 2] {
        f.niri_focus_output(n);
        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);
        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.set_size(100, 100);
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let state = &mut f.client(id).state;
    let seat = state.seats.keys().next().unwrap().clone();
    let qh = state.qh.clone();
    let text_input_manager = state.text_input_manager.as_ref().unwrap();
    let text_input = text_input_manager.get_text_input(&seat, &qh, ());
    let input_method_manager = state.input_method_manager.as_ref().unwrap();
    let input_method = input_method_manager.get_input_method(&seat, &qh, ());

    // Typing into the window on output 1 shows the input method popup there.
    f.niri_focus_output(1);
    f.double_roundtrip(id);
    text_input.enable();
    text_input.commit();
    let state = &mut f.client(id).state;
    let popup = state.create_surface();
    input_method.get_input_popup_surface(&popup, &qh, ());
    state.attach_pixel_buffer(&popup);
    popup.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_output_names(&mut f, id, &popup), ["headless-1"]);

    // Then typing into the window on output 2 moves the popup there.
    f.niri_focus_output(2);
    f.double_roundtrip(id);
    text_input.enable();
    text_input.commit();
    f.double_roundtrip(id);
    assert_eq!(entered_output_names(&mut f, id, &popup), ["headless-2"]);
}
