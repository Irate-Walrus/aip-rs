//! Golden tests for [`aip_codegen::generate`], called directly (no `protoc`
//! process). The generated source is compared byte-for-byte against committed
//! fixtures under `tests/golden/`; the same fixtures are compiled and
//! round-tripped in `roundtrip.rs`, so they are real, working code.
//!
//! Regenerate the fixtures after an intentional change with `BLESS=1`:
//!
//! ```sh
//! BLESS=1 cargo test -p aip-codegen --test golden
//! ```

use std::path::Path;

use aip_codegen::{generate, GenInput};
use aip_reflect::{RequestDescriptor, ResourceDescriptor};

/// A pagination-shaped [`RequestDescriptor`], with or without the `skip` field.
fn page_request(message_name: &str, has_skip: bool) -> RequestDescriptor {
    RequestDescriptor {
        message_name: message_name.to_owned(),
        has_page_token: true,
        has_page_size: true,
        has_skip,
        has_order_by: false,
        has_filter: false,
    }
}

/// The freight inputs, mirroring `examples/freight-server`: `shipper.proto`
/// (one variable) and `site.proto` (a parent + child variable) declare the
/// resources, and `freight_service.proto` — no resources at all — carries the
/// paginated List requests (`ListSitesRequest` with `skip`, `ListShippersRequest`
/// without).
fn freight_inputs() -> Vec<GenInput> {
    vec![
        GenInput {
            proto_file: "einride/example/freight/v1/shipper.proto".to_owned(),
            resources: vec![ResourceDescriptor {
                resource_type: "freight-example.einride.tech/Shipper".to_owned(),
                patterns: vec!["shippers/{shipper}".to_owned()],
            }],
            requests: vec![],
        },
        GenInput {
            proto_file: "einride/example/freight/v1/site.proto".to_owned(),
            resources: vec![ResourceDescriptor {
                resource_type: "freight-example.einride.tech/Site".to_owned(),
                patterns: vec!["shippers/{shipper}/sites/{site}".to_owned()],
            }],
            requests: vec![],
        },
        GenInput {
            proto_file: "einride/example/freight/v1/freight_service.proto".to_owned(),
            resources: vec![],
            requests: vec![
                page_request("ListShippersRequest", false),
                page_request("ListSitesRequest", true),
                // A non-paginated request contributes nothing.
                RequestDescriptor {
                    message_name: "GetShipperRequest".to_owned(),
                    has_page_token: false,
                    has_page_size: false,
                    has_skip: false,
                    has_order_by: false,
                    has_filter: false,
                },
            ],
        },
    ]
}

#[test]
fn freight_resources_match_golden() {
    let files = generate(&freight_inputs()).expect("freight inputs generate");
    assert_eq!(
        files.len(),
        3,
        "one output file per contributing proto file"
    );

    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let bless = std::env::var_os("BLESS").is_some();

    for file in &files {
        let path = golden_dir.join(&file.path);
        if bless {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &file.content).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "read golden {} ({e}); run with BLESS=1 to create it",
                path.display()
            )
        });
        assert_eq!(
            file.content, expected,
            "generated {} drifted from golden; run BLESS=1 to update",
            file.path,
        );
    }
}

#[test]
fn output_path_swaps_proto_for_aip_rs() {
    let files = generate(&freight_inputs()).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "einride/example/freight/v1/shipper.aip.rs",
            "einride/example/freight/v1/site.aip.rs",
            "einride/example/freight/v1/freight_service.aip.rs",
        ]
    );
}

#[test]
fn resource_without_patterns_produces_no_file() {
    let files = generate(&[GenInput {
        proto_file: "x/y.proto".to_owned(),
        resources: vec![ResourceDescriptor {
            resource_type: "example.com/Thing".to_owned(),
            patterns: vec![],
        }],
        requests: vec![],
    }])
    .unwrap();
    assert!(files.is_empty(), "a patternless resource emits nothing");
}

/// `page_token` + `page_size` is the qualifying shape — a request with only one
/// of the two (or its presence bools zeroed by a disabled plugin flag) emits no
/// impl, and a file with no other content emits no file at all.
#[test]
fn request_without_the_pagination_shape_produces_no_file() {
    let mut token_only = page_request("ListThingsRequest", false);
    token_only.has_page_size = false;
    let files = generate(&[GenInput {
        proto_file: "x/y.proto".to_owned(),
        resources: vec![],
        requests: vec![token_only],
    }])
    .unwrap();
    assert!(files.is_empty(), "half the pagination shape emits nothing");
}

#[test]
fn resource_type_without_a_type_name_is_an_error() {
    let err = generate(&[GenInput {
        proto_file: "x/y.proto".to_owned(),
        resources: vec![ResourceDescriptor {
            resource_type: "no-slash".to_owned(),
            patterns: vec!["things/{thing}".to_owned()],
        }],
        requests: vec![],
    }])
    .expect_err("a type with no `service/Type` form cannot name a struct");
    assert!(err.to_string().contains("no-slash"), "{err}");
}

#[test]
fn wildcard_pattern_is_rejected() {
    let err = generate(&[GenInput {
        proto_file: "x/y.proto".to_owned(),
        resources: vec![ResourceDescriptor {
            resource_type: "example.com/Thing".to_owned(),
            patterns: vec!["things/-".to_owned()],
        }],
        requests: vec![],
    }])
    .expect_err("a wildcard is not a valid pattern");
    // Surfaced from the runtime `aip_resourcename::Pattern::parse`.
    assert!(err.to_string().contains("wildcard"), "{err}");
}
