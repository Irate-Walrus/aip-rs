//! Enumerate the `ReflectMessage`-earning messages of a file: the freight
//! example proves a real `map<…>` field's synthetic entry is skipped (via the
//! shared `test-fixtures` pool), and a synthesized file proves nested messages
//! recurse parent-before-child with the right package-relative path (ADR-0009).

use aip_reflect::{reflect_messages_in_file, ReflectMessageName};
use prost_reflect::prost_types::{DescriptorProto, FileDescriptorProto};
use prost_reflect::DescriptorPool;

#[test]
fn walks_top_level_messages_and_skips_map_entries() {
    // `Site` carries `map<string, string> annotations`, whose synthetic
    // `Site.AnnotationsEntry` message has no generated struct — it must not earn
    // an impl. `Site.State` is an enum, so it is never a message here.
    let site = test_fixtures::message_descriptor("einride.example.freight.v1.Site")
        .expect("Site is in the fixture pool");
    let messages = reflect_messages_in_file(&site.parent_file());

    assert!(
        messages.contains(&ReflectMessageName {
            full_name: "einride.example.freight.v1.Site".to_owned(),
            path: vec!["Site".to_owned()],
        }),
        "the Site message earns an impl: {messages:?}",
    );
    assert!(
        !messages.iter().any(|m| m.full_name.ends_with("Entry")),
        "synthetic map-entry messages are skipped: {messages:?}",
    );
}

#[test]
fn recurses_nested_messages_parent_before_child() {
    // Synthesize `message Outer { message Inner { message Leaf {} } }` directly:
    // the fixture protos have no nested messages, so the recursion + path-build is
    // exercised here.
    let file = FileDescriptorProto {
        name: Some("nested.proto".to_owned()),
        package: Some("nested.v1".to_owned()),
        syntax: Some("proto3".to_owned()),
        message_type: vec![DescriptorProto {
            name: Some("Outer".to_owned()),
            nested_type: vec![DescriptorProto {
                name: Some("Inner".to_owned()),
                nested_type: vec![DescriptorProto {
                    name: Some("Leaf".to_owned()),
                    ..Default::default()
                }],
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
        .get_file_by_name("nested.proto")
        .expect("the synthesized file is in the pool");

    let messages = reflect_messages_in_file(&file);
    assert_eq!(
        messages,
        vec![
            ReflectMessageName {
                full_name: "nested.v1.Outer".to_owned(),
                path: vec!["Outer".to_owned()],
            },
            ReflectMessageName {
                full_name: "nested.v1.Outer.Inner".to_owned(),
                path: vec!["Outer".to_owned(), "Inner".to_owned()],
            },
            ReflectMessageName {
                full_name: "nested.v1.Outer.Inner.Leaf".to_owned(),
                path: vec!["Outer".to_owned(), "Inner".to_owned(), "Leaf".to_owned()],
            },
        ],
        "nested messages recurse parent-before-child with package-relative paths",
    );
}
