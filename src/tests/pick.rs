use smithay::input::pointer::{Focus, GrabStartData as PointerGrabStartData};
use smithay::utils::SERIAL_COUNTER;
use wayland_client::protocol::wl_pointer;

use super::client::ClientId;
use super::*;
use crate::input::pick_window_grab::PickWindowGrab;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

fn click(f: &mut Fixture, id: ClientId, button: u32) {
    let client = f.client(id);
    let manager = client.state.virtual_pointer_manager.as_ref().unwrap();
    let vp = manager.create_virtual_pointer(None, &client.qh, ());
    vp.button(0, button, wl_pointer::ButtonState::Pressed);
    vp.frame();
    vp.button(0, button, wl_pointer::ButtonState::Released);
    vp.frame();
    vp.destroy();
    f.double_roundtrip(id);
}

/// Picks with a left click, then checks that a right click afterwards ends its click grab, which
/// it doesn't if the pick left the left button pressed in smithay.
fn check_click_after_pick(f: &mut Fixture, id: ClientId) {
    click(f, id, BTN_LEFT);
    let pointer = f.niri().seat.get_pointer().unwrap();
    assert!(!pointer.is_grabbed(), "the pick grab must end on release");

    click(f, id, BTN_RIGHT);
    assert!(!pointer.is_grabbed(), "the click grab must end on release");
}

#[test]
fn pick_window_click_is_released() {
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
    f.niri_complete_animations();

    // Put the pointer at the center of the window, so that the pick returns it.
    let output = f.niri_output(1);
    let niri = f.niri();
    let output_loc = niri.global_space.output_geometry(&output).unwrap().loc;
    let ws = niri.layout.active_workspace().unwrap();
    let (tile, tile_pos, _) = ws.tiles_with_render_positions().next().unwrap();
    let window_id = tile.window().id();
    let center = output_loc.to_f64() + tile_pos + tile.tile_size().downscale(2.).to_point();
    niri.seat.get_pointer().unwrap().set_location(center);

    let (tx, rx) = async_channel::bounded(1);
    let state = f.niri_state();
    let pointer = state.niri.seat.get_pointer().unwrap();
    let start_data = PointerGrabStartData {
        focus: None,
        button: 0,
        location: pointer.current_location(),
    };
    let grab = PickWindowGrab::new(start_data);
    pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    state.niri.pick_window = Some(tx);

    check_click_after_pick(&mut f, id);
    assert_eq!(rx.try_recv(), Ok(Some(window_id)));
}

#[test]
fn pick_color_click_is_released() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    f.roundtrip(id);

    let (tx, rx) = async_channel::bounded(1);
    f.niri_state().handle_pick_color(tx);

    check_click_after_pick(&mut f, id);
    // There's no renderer, so no color.
    assert!(matches!(rx.try_recv(), Ok(None)));
}
