//! Enumerate the `google.api.resource` descriptors declared in the freight
//! example protos, via the shared `test-fixtures` descriptor pool.

use aip_reflect::{resource_descriptors_in_file, resource_descriptors_in_package};

const FREIGHT_PACKAGE: &str = "einride.example.freight.v1";

/// Looks up a resource's patterns by type among the given descriptors.
fn patterns_of<'a>(
    descriptors: &'a [aip_reflect::ResourceDescriptor],
    resource_type: &str,
) -> Option<&'a [String]> {
    descriptors
        .iter()
        .find(|d| d.resource_type == resource_type)
        .map(|d| d.patterns.as_slice())
}

#[test]
fn enumerates_all_freight_resources_in_package() {
    let pool = test_fixtures::pool();
    let resources = resource_descriptors_in_package(&pool, FREIGHT_PACKAGE);

    assert_eq!(
        patterns_of(&resources, "freight-example.einride.tech/Shipper"),
        Some(["shippers/{shipper}".to_owned()].as_slice()),
    );
    assert_eq!(
        patterns_of(&resources, "freight-example.einride.tech/Site"),
        Some(["shippers/{shipper}/sites/{site}".to_owned()].as_slice()),
    );
    assert_eq!(
        patterns_of(&resources, "freight-example.einride.tech/Shipment"),
        Some(["shippers/{shipper}/shipments/{shipment}".to_owned()].as_slice()),
    );
}

#[test]
fn unknown_package_yields_nothing() {
    let pool = test_fixtures::pool();
    assert!(resource_descriptors_in_package(&pool, "no.such.package.v1").is_empty());
}

#[test]
fn records_owning_message_name_and_delete_time_for_soft_deletable_resources() {
    let pool = test_fixtures::pool();
    let resources = resource_descriptors_in_package(&pool, FREIGHT_PACKAGE);

    // Each freight resource carries a `google.protobuf.Timestamp delete_time`, so
    // it is soft-deletable; its `message_name` is the prost struct the codegen
    // plugin lands the `SoftDeletable` impl on.
    for (resource_type, message_name) in [
        ("freight-example.einride.tech/Shipper", "Shipper"),
        ("freight-example.einride.tech/Site", "Site"),
        ("freight-example.einride.tech/Shipment", "Shipment"),
    ] {
        let descriptor = resources
            .iter()
            .find(|d| d.resource_type == resource_type)
            .unwrap_or_else(|| panic!("{resource_type} is declared in the freight package"));
        assert_eq!(descriptor.message_name.as_deref(), Some(message_name));
        assert!(
            descriptor.has_delete_time,
            "{resource_type} carries a Timestamp delete_time",
        );
    }
}

#[test]
fn enumerates_resource_declared_in_a_single_file() {
    // `shipper.proto` declares exactly the Shipper resource.
    let shipper = test_fixtures::message_descriptor("einride.example.freight.v1.Shipper")
        .expect("Shipper is in the fixture pool");
    let resources = resource_descriptors_in_file(&shipper.parent_file());

    assert_eq!(
        patterns_of(&resources, "freight-example.einride.tech/Shipper"),
        Some(["shippers/{shipper}".to_owned()].as_slice()),
    );
}
