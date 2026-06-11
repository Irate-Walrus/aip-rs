//! Digest the AIP-standard request fields of the freight example protos into
//! `RequestDescriptor`s (via the shared `test-fixtures` descriptor pool), and
//! check the near-miss shapes (wrong type, proto3-`optional`, repeated) that
//! must silently not count (ADR-0013).

use aip_reflect::{request_descriptors_in_file, RequestDescriptor};
use prost_reflect::prost_types::{
    field_descriptor_proto, DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    OneofDescriptorProto,
};
use prost_reflect::DescriptorPool;

/// Looks up a message's descriptor by simple name among the given descriptors.
fn descriptor_of<'a>(
    descriptors: &'a [RequestDescriptor],
    message_name: &str,
) -> &'a RequestDescriptor {
    descriptors
        .iter()
        .find(|d| d.message_name == message_name)
        .unwrap_or_else(|| panic!("{message_name} has a request descriptor"))
}

#[test]
fn digests_the_freight_list_requests() {
    let message = test_fixtures::message_descriptor("einride.example.freight.v1.ListSitesRequest")
        .expect("ListSitesRequest is in the fixture pool");
    let requests = request_descriptors_in_file(&message.parent_file());

    // `page_size` + `page_token` only.
    assert_eq!(
        descriptor_of(&requests, "ListShippersRequest"),
        &RequestDescriptor {
            message_name: "ListShippersRequest".to_owned(),
            has_page_token: true,
            has_page_size: true,
            has_skip: false,
            has_order_by: false,
            has_filter: false,
        },
    );
    // The full AIP-158/-132/-160 set, including `skip`.
    assert_eq!(
        descriptor_of(&requests, "ListSitesRequest"),
        &RequestDescriptor {
            message_name: "ListSitesRequest".to_owned(),
            has_page_token: true,
            has_page_size: true,
            has_skip: true,
            has_order_by: true,
            has_filter: true,
        },
    );
    // Pagination only (the fixture protos track upstream einride, where
    // `ListShipmentsRequest` has no `filter`).
    assert_eq!(
        descriptor_of(&requests, "ListShipmentsRequest"),
        &RequestDescriptor {
            message_name: "ListShipmentsRequest".to_owned(),
            has_page_token: true,
            has_page_size: true,
            has_skip: false,
            has_order_by: false,
            has_filter: false,
        },
    );
    // A non-List request carries none of the fields.
    assert_eq!(
        descriptor_of(&requests, "GetShipperRequest"),
        &RequestDescriptor {
            message_name: "GetShipperRequest".to_owned(),
            has_page_token: false,
            has_page_size: false,
            has_skip: false,
            has_order_by: false,
            has_filter: false,
        },
    );
}

/// A name match with the wrong type, a proto3-`optional` field, or a repeated
/// field is silently `false` — the message just qualifies for fewer traits.
#[test]
fn near_miss_fields_do_not_count() {
    // Build `message NearMiss { int32 page_token = 1; optional int32 page_size = 2;
    // repeated string filter = 3; string order_by = 4; }` directly — the fixture
    // protos are well-formed, so the near-miss shapes are synthesized here.
    let file = FileDescriptorProto {
        name: Some("near_miss.proto".to_owned()),
        package: Some("near.miss.v1".to_owned()),
        syntax: Some("proto3".to_owned()),
        message_type: vec![DescriptorProto {
            name: Some("NearMiss".to_owned()),
            field: vec![
                FieldDescriptorProto {
                    name: Some("page_token".to_owned()),
                    number: Some(1),
                    r#type: Some(field_descriptor_proto::Type::Int32 as i32),
                    label: Some(field_descriptor_proto::Label::Optional as i32),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("page_size".to_owned()),
                    number: Some(2),
                    r#type: Some(field_descriptor_proto::Type::Int32 as i32),
                    label: Some(field_descriptor_proto::Label::Optional as i32),
                    proto3_optional: Some(true),
                    oneof_index: Some(0),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("filter".to_owned()),
                    number: Some(3),
                    r#type: Some(field_descriptor_proto::Type::String as i32),
                    label: Some(field_descriptor_proto::Label::Repeated as i32),
                    ..Default::default()
                },
                FieldDescriptorProto {
                    name: Some("order_by".to_owned()),
                    number: Some(4),
                    r#type: Some(field_descriptor_proto::Type::String as i32),
                    label: Some(field_descriptor_proto::Label::Optional as i32),
                    ..Default::default()
                },
            ],
            oneof_decl: vec![OneofDescriptorProto {
                name: Some("_page_size".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_proto(file)
        .expect("the synthesized file is well-formed");
    let file = pool
        .get_file_by_name("near_miss.proto")
        .expect("the synthesized file is in the pool");

    let requests = request_descriptors_in_file(&file);
    assert_eq!(
        descriptor_of(&requests, "NearMiss"),
        &RequestDescriptor {
            message_name: "NearMiss".to_owned(),
            has_page_token: false, // wrong type (int32, not string)
            has_page_size: false,  // proto3-`optional` -> prost `Option<i32>`
            has_skip: false,       // absent
            has_order_by: true,    // well-formed -> counts
            has_filter: false,     // repeated, not singular
        },
    );
}
