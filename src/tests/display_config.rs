use std::collections::HashMap;
use std::str::FromStr as _;

use niri_config::{Config, FloatOrInt};
use smithay::utils::Transform;

use super::*;
use crate::utils::PanelOrientation;

#[test]
fn display_config_keeps_entry_name_and_other_settings() {
    // The entry matches headless-1 by make, model and serial.
    let config = Config::parse_mem(
        r#"output "niri headless 1" { scale 2; variable-refresh-rate; focus-at-startup; }"#,
    )
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    // What ApplyMonitorsConfig sends for a scale change.
    let new = niri_config::Output {
        name: String::from("headless-1"),
        scale: Some(FloatOrInt(1.5)),
        position: Some(niri_config::Position { x: 0, y: 0 }),
        mode: Some(niri_config::output::Mode {
            custom: false,
            mode: niri_ipc::ConfiguredMode::from_str("1920x1080").unwrap(),
        }),
        ..Default::default()
    };
    let new_conf = HashMap::from([(String::from("headless-1"), Some(new))]);
    f.niri_state().on_display_config_msg(new_conf);

    assert_eq!(f.niri_output(1).current_scale().fractional_scale(), 1.5);
    let config = f.niri().config.borrow();
    let [output] = &config.outputs.0[..] else {
        panic!("{:?}", config.outputs);
    };
    assert_eq!(output.name, "niri headless 1");
    assert!(output.variable_refresh_rate.is_some());
    assert!(output.focus_at_startup);
}

#[test]
fn display_config_keeps_a_custom_mode_sent_back() {
    let config =
        Config::parse_mem(r#"output "headless-1" { mode custom=true "1920x1080@50"; }"#).unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    // What ApplyMonitorsConfig sends for a scale change when the client sent back the current
    // mode: no mode.
    let new = niri_config::Output {
        name: String::from("headless-1"),
        scale: Some(FloatOrInt(1.5)),
        position: Some(niri_config::Position { x: 0, y: 0 }),
        mode: None,
        ..Default::default()
    };
    let new_conf = HashMap::from([(String::from("headless-1"), Some(new))]);
    f.niri_state().on_display_config_msg(new_conf);

    let config = f.niri().config.borrow();
    let [entry] = &config.outputs.0[..] else {
        panic!("{:?}", config.outputs);
    };
    assert!(entry.mode.as_ref().is_some_and(|mode| mode.custom));
}

#[test]
fn display_config_keeps_the_panel_orientation_out_of_the_config() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    // A panel mounted upside down.
    let output = f.niri_output(1);
    output
        .user_data()
        .insert_if_missing(|| PanelOrientation(Transform::_180));
    f.niri_state().reload_output_config();
    assert_eq!(output.current_transform(), Transform::_180);

    // A client that changes only the scale sends back the transform it got, which is _180.
    let new = niri_config::Output {
        name: String::from("headless-1"),
        scale: Some(FloatOrInt(1.5)),
        transform: niri_ipc::Transform::_180,
        position: Some(niri_config::Position { x: 0, y: 0 }),
        mode: None,
        ..Default::default()
    };
    let new_conf = HashMap::from([(String::from("headless-1"), Some(new))]);
    f.niri_state().on_display_config_msg(new_conf);

    assert_eq!(output.current_transform(), Transform::_180);
    let config = f.niri().config.borrow();
    let [entry] = &config.outputs.0[..] else {
        panic!("{:?}", config.outputs);
    };
    assert_eq!(entry.transform, niri_ipc::Transform::Normal);
}
