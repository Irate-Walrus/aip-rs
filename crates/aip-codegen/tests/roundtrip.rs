//! Compile the golden fixtures from `golden.rs` and exercise them — proving the
//! generated wrappers are real, working code that round-trips parse <-> format
//! over the runtime `aip_resourcename::Pattern` API (acceptance criterion).

mod shipper {
    include!("golden/einride/example/freight/v1/shipper.aip.rs");
}
mod site {
    include!("golden/einride/example/freight/v1/site.aip.rs");
}

use shipper::ShipperResourceName;
use site::SiteResourceName;

#[test]
fn single_variable_wrapper_round_trips() {
    assert_eq!(
        ShipperResourceName::TYPE,
        "freight-example.einride.tech/Shipper"
    );
    assert_eq!(ShipperResourceName::PATTERN, "shippers/{shipper}");

    let name = ShipperResourceName::parse("shippers/acme").expect("a valid shipper name");
    assert_eq!(name.shipper, "acme");
    assert_eq!(
        name.format().expect("format a complete name"),
        "shippers/acme"
    );
}

#[test]
fn multi_variable_wrapper_round_trips() {
    assert_eq!(SiteResourceName::TYPE, "freight-example.einride.tech/Site");
    assert_eq!(SiteResourceName::PATTERN, "shippers/{shipper}/sites/{site}");

    let name = SiteResourceName::parse("shippers/acme/sites/dock-1").expect("a valid site name");
    assert_eq!(name.shipper, "acme");
    assert_eq!(name.site, "dock-1");
    assert_eq!(
        name.format().expect("format a complete name"),
        "shippers/acme/sites/dock-1",
    );
}

#[test]
fn parse_rejects_a_name_that_does_not_match_the_pattern() {
    // A Shipper name is not a Site name.
    assert!(SiteResourceName::parse("shippers/acme").is_err());
    // A Site name is not a Shipper name (too many segments).
    assert!(ShipperResourceName::parse("shippers/acme/sites/dock-1").is_err());
}

#[test]
fn format_round_trips_from_constructed_values() {
    let name = SiteResourceName {
        shipper: "acme".to_owned(),
        site: "dock-1".to_owned(),
    };
    let formatted = name.format().expect("format");
    assert_eq!(SiteResourceName::parse(&formatted).unwrap(), name);
}
