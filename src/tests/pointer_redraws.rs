use std::thread::sleep;
use std::time::Duration;

use smithay::input::pointer::CursorIcon;
use smithay::output::Output;
use smithay::utils::{Logical, Point};

use super::*;
use crate::niri::{Niri, PointerVisibility, RedrawState};

/// Runs the pointer redraw refresh and returns which outputs it queued a redraw on.
fn refresh(niri: &mut Niri, outputs: &[&Output; 2]) -> [bool; 2] {
    for state in niri.output_state.values_mut() {
        state.redraw_state = RedrawState::Idle;
    }
    niri.refresh_pointer_redraws();
    queued(niri, outputs)
}

fn queued(niri: &Niri, outputs: &[&Output; 2]) -> [bool; 2] {
    outputs.map(|output| matches!(niri.output_state[output].redraw_state, RedrawState::Queued))
}

fn move_pointer(niri: &mut Niri, pos: Point<f64, Logical>) {
    niri.seat.get_pointer().unwrap().set_location(pos);
}

#[test]
fn cursor_redraws_only_outputs_it_is_drawn_on() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let output1 = f.niri_output(1);
    let output2 = f.niri_output(2);
    let outputs = [&output1, &output2];

    let niri = f.niri();
    let geo1 = niri
        .global_space
        .output_geometry(&output1)
        .unwrap()
        .to_f64();
    let geo2 = niri
        .global_space
        .output_geometry(&output2)
        .unwrap()
        .to_f64();
    // The test relies on output 2 being to the right of output 1.
    assert_eq!(geo2.loc, geo1.loc + Point::from((geo1.size.w, 0.)));

    let center1 = geo1.loc + geo1.size.downscale(2.).to_point();
    let center2 = geo2.loc + geo2.size.downscale(2.).to_point();
    let edge1 = Point::from((geo2.loc.x - 0.5, center1.y));

    move_pointer(niri, center1);
    assert_eq!(refresh(niri, &outputs), [true, false]);
    assert_eq!(refresh(niri, &outputs), [false, false]);

    // Motion within one output doesn't redraw the others.
    move_pointer(niri, center1 + Point::from((10., 10.)));
    assert_eq!(refresh(niri, &outputs), [true, false]);

    // A cursor straddling the edge is drawn on both outputs.
    move_pointer(niri, edge1);
    assert_eq!(refresh(niri, &outputs), [true, true]);
    move_pointer(niri, center1);
    assert_eq!(refresh(niri, &outputs), [true, true]);

    // The tablet cursor is drawn instead of the pointer, and must be cleared from its output.
    niri.tablet_cursor_location = Some(center2);
    assert_eq!(refresh(niri, &outputs), [true, true]);
    niri.tablet_cursor_location = None;
    assert_eq!(refresh(niri, &outputs), [true, true]);

    // Cursor surface commits redraw only the outputs showing the cursor.
    for state in niri.output_state.values_mut() {
        state.redraw_state = RedrawState::Idle;
    }
    niri.queue_redraw_cursor_outputs();
    assert_eq!(queued(niri, &outputs), [true, false]);

    // Hiding the cursor erases it.
    niri.pointer_visibility = PointerVisibility::Hidden;
    assert_eq!(refresh(niri, &outputs), [true, false]);
    for state in niri.output_state.values_mut() {
        state.redraw_state = RedrawState::Idle;
    }
    niri.queue_redraw_cursor_outputs();
    assert_eq!(queued(niri, &outputs), [false, false]);
}

#[test]
fn cursor_wider_than_output_redraws_on_motion() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (8, 8));
    let output1 = f.niri_output(1);
    let output2 = f.niri_output(2);
    let outputs = [&output1, &output2];

    let niri = f.niri();
    let geo2 = niri
        .global_space
        .output_geometry(&output2)
        .unwrap()
        .to_f64();
    let bbox = niri.cursor_manager.current_cursor_bbox(1).unwrap();
    // The test relies on the cursor covering the whole output with room to move.
    assert!(bbox.size.w >= geo2.size.w + 2. && bbox.size.h >= geo2.size.h + 2.);

    // Put the cursor's top-left corner at the output's top-left corner.
    let pos = geo2.loc - bbox.loc;
    move_pointer(niri, pos);
    assert!(refresh(niri, &outputs)[1]);

    // The part of the cursor on the output moves even though it still covers the whole output.
    move_pointer(niri, pos - Point::from((1., 1.)));
    assert!(refresh(niri, &outputs)[1]);
}

#[test]
fn ending_spatial_movement_grab_redraws_cursor_output() {
    use smithay::backend::input::InputTime;
    use smithay::input::pointer::{CursorIcon, CursorImageStatus, Focus, GrabStartData};
    use smithay::utils::SERIAL_COUNTER;

    use crate::input::spatial_movement_grab::SpatialMovementGrab;

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let output1 = f.niri_output(1);
    let output2 = f.niri_output(2);
    let outputs = [&output1, &output2];

    // The cursor is on output 2.
    let niri = f.niri();
    let geo2 = niri
        .global_space
        .output_geometry(&output2)
        .unwrap()
        .to_f64();
    let center2 = geo2.loc + geo2.size.downscale(2.).to_point();
    move_pointer(niri, center2);
    refresh(niri, &outputs);

    // A gesture on output 1 shows the AllScroll cursor.
    let ws_id = niri
        .layout
        .monitor_for_output(&output1)
        .unwrap()
        .active_workspace_ref()
        .id();
    let start_data = GrabStartData {
        focus: None,
        button: 0x112,
        location: center2,
    };
    let grab = SpatialMovementGrab::new(start_data, output1.clone(), ws_id, false);
    let state = f.niri_state();
    let pointer = state.niri.seat.get_pointer().unwrap();
    pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    state
        .niri
        .cursor_manager
        .set_cursor_image(CursorImageStatus::Named(CursorIcon::AllScroll));

    // Ending it resets the cursor, which must be redrawn on output 2.
    for state in state.niri.output_state.values_mut() {
        state.redraw_state = RedrawState::Idle;
    }
    pointer.unset_grab(state, SERIAL_COUNTER.next_serial(), InputTime::now());
    assert_eq!(queued(&state.niri, &outputs), [false, true]);
}

#[test]
fn animated_cursor_redraws_when_its_frame_changes() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.niri_output(1);

    // The default cursor, with two frames of 20 ms each.
    let niri = f.niri();
    niri.cursor_manager
        .insert_animated_cursor(CursorIcon::Default, 1, &[20, 20]);
    let geo = niri.global_space.output_geometry(&output).unwrap().to_f64();
    move_pointer(niri, geo.loc + geo.size.downscale(2.).to_point());
    f.state.server.dispatch();

    // The output isn't redrawn on every refresh cycle, but when the next frame is due.
    let state = &f.niri().output_state[&output];
    assert!(!state.unfinished_animations_remain);
    assert!(state.cursor_frame_timer.is_some());
    let sequence = state.frame_callback_sequence;

    sleep(Duration::from_millis(30));
    f.state.server.dispatch();
    let state = &f.niri().output_state[&output];
    assert_ne!(state.frame_callback_sequence, sequence);
    assert!(state.cursor_frame_timer.is_some());

    // A skipped render that queues no vblank doesn't arm the timer again, so the output doesn't
    // keep redrawing. The headless backend never skips a render, so skip it by turning the
    // monitors off. (A render skipped after an error rendering or queueing the frame arms it, but
    // only the TTY backend has those errors.)
    f.niri().monitors_active = false;
    sleep(Duration::from_millis(30));
    f.state.server.dispatch();
    assert!(f.niri().output_state[&output].cursor_frame_timer.is_none());
}
