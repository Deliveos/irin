use std::collections::HashMap;
use std::str::FromStr as _;

use niri_config::{Config, FloatOrInt};

use super::*;

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
