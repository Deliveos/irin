use smithay::utils::{Logical, Physical, Rectangle, Scale, Size};

use super::*;
use crate::utils::output_size;

/// Maps a 100 × 100 window with a subsurface at (16000, 16000) on a 1920 × 1080 output, and
/// returns its window geometry and its cast area.
fn far_subsurface_cast_bbox(
    set_geometry: bool,
) -> (Rectangle<i32, Logical>, Rectangle<i32, Physical>) {
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

    let (_, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_position(16000, 16000);
    let window = f.client(id).window(&surface);
    if set_geometry {
        window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    }
    window.commit();
    f.double_roundtrip(id);

    let output = f.niri_output(1);
    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    assert!(mapped.window.bbox_with_popups().size.w > 16000);

    let geo = mapped.window.geometry();
    let bbox = mapped.cast_bbox(Scale::from(1.), output_size(&output));
    (geo, bbox)
}

#[test]
fn window_cast_area_is_cropped_around_the_window() {
    let (geo, bbox) = far_subsurface_cast_bbox(true);
    assert_eq!(geo, Rectangle::from_size(Size::from((100, 100))));

    // The cast is at most the output's size, and starts at the window.
    assert_eq!(bbox, Rectangle::from_size(Size::from((1920, 1080))));
}

#[test]
fn window_cast_area_is_cropped_without_window_geometry() {
    // Without a window geometry, the geometry is the bounding box of the surface and its
    // subsurfaces, so it reaches the far subsurface.
    let (geo, bbox) = far_subsurface_cast_bbox(false);
    assert_eq!(geo, Rectangle::from_size(Size::from((16001, 16001))));

    // The cast is still at most the output's size, from the start of the window geometry.
    assert_eq!(bbox, Rectangle::from_size(Size::from((1920, 1080))));
}
