//! Compile the golden fixtures from `golden.rs` and exercise them — proving the
//! generated wrappers are real, working code: validated `new`, infallible
//! `Display`, `FromStr`/`parse` round-tripping over the runtime
//! `aip_resourcename::Pattern` API, and a typed `parent()` accessor.
//!
//! The fixtures are mounted one module per file with the wrappers re-exported
//! flat at this module's root, the convention the generated `parent()` relies on
//! (`super::ShipperResourceName`) and that `examples/freight-server` follows.

mod shipper {
    include!("golden/einride/example/freight/v1/shipper.aip.rs");
}
mod site {
    include!("golden/einride/example/freight/v1/site.aip.rs");
}

use std::str::FromStr;

// Re-export flat so each wrapper is reachable as `super::<Name>` from the other
// wrapper's module — what the generated `SiteResourceName::parent()` references.
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
    assert_eq!(name.shipper(), "acme");
    // Infallible Display, and the `From<_> for String` built on it.
    assert_eq!(name.to_string(), "shippers/acme");
    assert_eq!(String::from(name.clone()), "shippers/acme");
}

#[test]
fn validated_new_rejects_an_invalid_variable() {
    let name = ShipperResourceName::new("acme").expect("a single-segment id is valid");
    assert_eq!(name.to_string(), "shippers/acme");
    // An embedded `/` is two segments, not one — rejected at construction.
    assert!(ShipperResourceName::new("acme/dock-1").is_err());
    // Empty is rejected.
    assert!(ShipperResourceName::new("").is_err());
}

#[test]
fn from_str_delegates_to_parse() {
    let name: ShipperResourceName = "shippers/acme".parse().expect("a valid shipper name");
    assert_eq!(name.shipper(), "acme");
    assert!(ShipperResourceName::from_str("not-a-shipper").is_err());
}

#[test]
fn multi_variable_wrapper_round_trips() {
    assert_eq!(SiteResourceName::TYPE, "freight-example.einride.tech/Site");
    assert_eq!(SiteResourceName::PATTERN, "shippers/{shipper}/sites/{site}");

    let name = SiteResourceName::parse("shippers/acme/sites/dock-1").expect("a valid site name");
    assert_eq!(name.shipper(), "acme");
    assert_eq!(name.site(), "dock-1");
    assert_eq!(name.to_string(), "shippers/acme/sites/dock-1");
}

#[test]
fn parse_rejects_a_name_that_does_not_match_the_pattern() {
    // A Shipper name is not a Site name.
    assert!(SiteResourceName::parse("shippers/acme").is_err());
    // A Site name is not a Shipper name (too many segments).
    assert!(ShipperResourceName::parse("shippers/acme/sites/dock-1").is_err());
}

#[test]
fn display_round_trips_from_constructed_values() {
    let name = SiteResourceName::new("acme", "dock-1").expect("valid site variables");
    let formatted = name.to_string();
    assert_eq!(SiteResourceName::parse(&formatted).unwrap(), name);
}

#[test]
fn parent_returns_the_typed_parent_wrapper() {
    let site = SiteResourceName::new("acme", "dock-1").expect("valid site variables");
    let parent: ShipperResourceName = site.parent();
    assert_eq!(parent, ShipperResourceName::new("acme").unwrap());
    assert_eq!(parent.to_string(), "shippers/acme");
}
