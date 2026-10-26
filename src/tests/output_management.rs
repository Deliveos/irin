use niri_config::{Config, FloatOrInt, OutputName, Outputs};

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
