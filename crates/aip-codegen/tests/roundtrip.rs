//! Compile the golden fixtures from `golden.rs` and exercise them — proving the
//! generated code is real, working code: validated `new`, infallible `Display`,
//! `FromStr`/`parse` round-tripping over the runtime `aip_resourcename::Pattern`
//! API, a typed `parent()` accessor, and the `PageRequest` / `OrderByRequest`
//! impls' accessors.
//!
//! The fixtures are `use`-free and fully path-qualified, mounted **directly in
//! the module that holds the message structs** (ADR-0013's one mount rule, as
//! `examples/freight-server`'s `proto.rs` does) — which is also what lets the
//! generated `parent()` name `ShipperResourceName`, and a `PageRequest` impl
//! its request struct, by bare path. Here that module is `freight`, with stub
//! request structs standing in for the prost-generated ones.

mod freight {
    /// Stands in for prost's `ListShippersRequest` — pagination fields, no `skip`.
    #[derive(Default)]
    pub struct ListShippersRequest {
        pub page_size: i32,
        pub page_token: String,
    }

    /// Stands in for prost's `ListSitesRequest` — pagination fields plus `skip`
    /// and an AIP-132 `order_by`.
    #[derive(Default)]
    pub struct ListSitesRequest {
        pub page_size: i32,
        pub page_token: String,
        pub skip: i32,
        pub order_by: String,
    }

    include!("golden/einride/example/freight/v1/shipper.aip.rs");
    include!("golden/einride/example/freight/v1/site.aip.rs");
    include!("golden/einride/example/freight/v1/freight_service.aip.rs");
}

use std::str::FromStr;

use aip_ordering::OrderByRequest;
use aip_pagination::PageRequest;
use freight::{ListShippersRequest, ListSitesRequest, ShipperResourceName, SiteResourceName};

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

/// A request without a `skip` field keeps the trait's `0` default.
#[test]
fn page_request_impl_reads_the_pagination_fields() {
    let request = ListShippersRequest {
        page_size: 25,
        page_token: "next".to_owned(),
    };
    assert_eq!(request.page_token(), "next");
    assert_eq!(request.page_size(), 25);
    assert_eq!(request.skip(), 0, "no `skip` field -> the trait default");
}

/// A request with a `skip` field gets the generated override.
#[test]
fn page_request_impl_overrides_skip_when_the_field_exists() {
    let request = ListSitesRequest {
        page_size: 10,
        page_token: String::new(),
        skip: 30,
        order_by: String::new(),
    };
    assert_eq!(request.page_token(), "");
    assert_eq!(request.page_size(), 10);
    assert_eq!(request.skip(), 30);
}

/// A request with an `order_by` field gets the generated `OrderByRequest` impl.
#[test]
fn order_by_request_impl_reads_the_order_by_field() {
    let request = ListSitesRequest {
        order_by: "display_name desc".to_owned(),
        ..Default::default()
    };
    assert_eq!(request.order_by(), "display_name desc");
}
