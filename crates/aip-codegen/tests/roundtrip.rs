//! Compile the golden fixtures from `golden.rs` and exercise them — proving the
//! generated code is real, working code: validated `new`, infallible `Display`,
//! `FromStr`/`parse` round-tripping over the runtime `aip_resourcename::Pattern`
//! API, a typed `parent()` accessor, `parse_field` / `mint` / `mint_under`
//! constructors, and the `PageRequest` / `OrderByRequest` impls' accessors.
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

    /// Stands in for prost's `Shipper` resource message — the `SoftDeletable`
    /// impl in `shipper.aip.rs` reads its `delete_time` presence. prost maps a
    /// `google.protobuf.Timestamp delete_time` to `Option<_>`, so a unit stand-in
    /// for the timestamp is enough to exercise `.is_some()`.
    #[derive(Default)]
    pub struct Shipper {
        pub delete_time: Option<()>,
    }

    /// Stands in for prost's `Site` resource message — also soft-deletable.
    #[derive(Default)]
    pub struct Site {
        pub delete_time: Option<()>,
    }

    include!("golden/einride/example/freight/v1/shipper.aip.rs");
    include!("golden/einride/example/freight/v1/site.aip.rs");
    include!("golden/einride/example/freight/v1/freight_service.aip.rs");
}

use std::str::FromStr;

use aip_ordering::OrderByRequest;
use aip_pagination::PageRequest;
use aip_softdelete::{check_visible, SoftDeletable, State};
use freight::{
    ListShippersRequest, ListSitesRequest, Shipper, ShipperResourceName, Site, SiteResourceName,
};

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

/// The wrapper stores the canonical name, so `as_str` / `AsRef<str>` borrow it
/// without re-formatting, and `From<_> for String` and `Display` agree with it.
#[test]
fn as_str_borrows_the_stored_canonical_name() {
    let name = SiteResourceName::new("acme", "dock-1").expect("valid site variables");
    assert_eq!(name.as_str(), "shippers/acme/sites/dock-1");
    // `AsRef<str>` exposes the same slice — drops the wrapper into `AsRef` APIs.
    let as_ref: &str = name.as_ref();
    assert_eq!(as_ref, "shippers/acme/sites/dock-1");
    // Display and `From<_> for String` reuse the same stored name.
    assert_eq!(name.to_string(), name.as_str());
    assert_eq!(String::from(name.clone()), "shippers/acme/sites/dock-1");
    // `as_str` borrows — calling it twice yields the same pointer (no re-format).
    assert!(std::ptr::eq(name.as_str(), name.as_str()));
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

/// `Ord` follows the canonical resource name's *string* order, not the variable
/// tuple. The two diverge when one variable value is a prefix of another: with
/// `shipper` = `a` vs `a-b`, `'-' (0x2D) < '/' (0x2F)`, so the full names sort
/// `shippers/a-b/sites/x` before `shippers/a/sites/x` — the opposite of what a
/// `(shipper, site)` tuple derive would give (`"a" < "a-b"`).
#[test]
fn ord_follows_string_order_not_the_variable_tuple() {
    let a = SiteResourceName::new("a", "x").expect("valid site variables");
    let ab = SiteResourceName::new("a-b", "x").expect("valid site variables");

    // String order over the canonical names: `a-b` first.
    assert!(ab.as_str() < a.as_str());
    assert!(ab < a, "Ord must follow the canonical name string order");

    // A field-tuple derive would order `("a", _) < ("a-b", _)` — the opposite —
    // so this asserts we are NOT deriving on the variable fields.
    assert!(
        (ab.shipper(), ab.site()) > (a.shipper(), a.site()),
        "the variable tuple sorts the other way, confirming the divergence",
    );

    // `Ord` agrees with the names a `BTreeMap<String, _>` would sort by.
    let mut names = [a.to_string(), ab.to_string()];
    names.sort();
    let mut wrappers = [a.clone(), ab.clone()];
    wrappers.sort();
    assert_eq!(
        wrappers.iter().map(|w| w.to_string()).collect::<Vec<_>>(),
        names.to_vec(),
    );
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

#[test]
fn parse_field_accepts_a_valid_name() {
    let name =
        ShipperResourceName::parse_field("name", "shippers/acme").expect("a valid shipper name");
    assert_eq!(name.shipper(), "acme");
}

#[test]
fn parse_field_rejects_empty_name_with_the_field_path() {
    let err = ShipperResourceName::parse_field("name", "").expect_err("empty name is rejected");
    assert_eq!(err.field, "name");
    // The inner error preserves specificity: Empty, not PatternMismatch.
    assert!(matches!(err.source, aip_resourcename::Error::Empty));
}

#[test]
fn parse_field_rejects_pattern_mismatch_with_the_field_path() {
    let err = ShipperResourceName::parse_field("parent", "shippers/acme/sites/dock-1")
        .expect_err("site name is rejected as a shipper name");
    assert_eq!(err.field, "parent");
    assert!(matches!(
        err.source,
        aip_resourcename::Error::PatternMismatch { .. }
    ));
}

#[test]
fn mint_returns_a_valid_shipper_name() {
    let a = ShipperResourceName::mint();
    let b = ShipperResourceName::mint();
    // Two minted names are distinct (UUIDs) and both parse correctly.
    assert_ne!(a.to_string(), b.to_string());
    assert!(ShipperResourceName::parse(a.as_str()).is_ok());
}

#[test]
fn mint_under_copies_parent_variables_and_mints_the_last() {
    let parent = ShipperResourceName::new("acme").expect("valid shipper");
    let site = SiteResourceName::mint_under(&parent);
    assert_eq!(site.shipper(), "acme");
    // The site variable is a UUID (non-empty, single segment).
    assert!(!site.site().is_empty());
    assert!(!site.site().contains('/'));
    // Two mints produce distinct names.
    let site2 = SiteResourceName::mint_under(&parent);
    assert_ne!(site.site(), site2.site());
}

/// The generated `SoftDeletable` impl reads `delete_time` presence as the
/// soft-delete state, and the blanket `From<&T>` lets `check_visible` take the
/// resource directly (the ergonomics issue #134 is about).
#[test]
fn soft_deletable_impl_drives_visibility_from_delete_time() {
    let live = Shipper { delete_time: None };
    let deleted = Shipper {
        delete_time: Some(()),
    };
    assert_eq!(live.soft_delete_state(), State::Live);
    assert_eq!(deleted.soft_delete_state(), State::Deleted);

    // No `State::from_deleted(...)` at the call site — the resource converts on
    // its own, for both resource messages.
    check_visible(&live, false, "shippers/acme").expect("a live shipper is visible");
    assert!(check_visible(&deleted, false, "shippers/acme").is_err());
    check_visible(&deleted, true, "shippers/acme").expect("show_deleted reveals it");

    let live_site = Site { delete_time: None };
    assert_eq!(live_site.soft_delete_state(), State::Live);
}
