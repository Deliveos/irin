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
