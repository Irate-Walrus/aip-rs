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

use aip_codegen::{generate, CratePaths, GenFile, GenInput};
use aip_reflect::{ReflectMessageName, RequestDescriptor, ResourceDescriptor};

/// Compare each generated file against its committed golden, or rewrite them all
/// under `BLESS=1`. Shared by the freight and reflect golden tests.
fn assert_matches_golden(files: &[GenFile]) {
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let bless = std::env::var_os("BLESS").is_some();
    for file in files {
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
/// paginated List requests (`ListSitesRequest` with `skip` and `order_by`,
/// `ListShippersRequest` without either).
fn freight_inputs() -> Vec<GenInput> {
    vec![
        // The Shipper message carries a `delete_time`, so it earns both a
        // resource-name wrapper and a `SoftDeletable` impl (ADR-0014).
        GenInput::new(
            "einride/example/freight/v1/shipper.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "freight-example.einride.tech/Shipper".to_owned(),
                patterns: vec!["shippers/{shipper}".to_owned()],
                message_name: Some("Shipper".to_owned()),
                has_delete_time: true,
            }],
            vec![],
            vec![],
        ),
        GenInput::new(
            "einride/example/freight/v1/site.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "freight-example.einride.tech/Site".to_owned(),
                patterns: vec!["shippers/{shipper}/sites/{site}".to_owned()],
                message_name: Some("Site".to_owned()),
                has_delete_time: true,
            }],
            vec![],
            vec![],
        ),
        GenInput::new(
            "einride/example/freight/v1/freight_service.proto".to_owned(),
            vec![],
            vec![
                page_request("ListShippersRequest", false),
                // `ListSites` honors an AIP-132 `order_by`, so it earns both the
                // `PageRequest` (with `skip`) and `OrderByRequest` impls.
                RequestDescriptor {
                    has_order_by: true,
                    ..page_request("ListSitesRequest", true)
                },
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
            vec![],
        ),
    ]
}

#[test]
fn freight_resources_match_golden() {
    let files =
        generate(&freight_inputs(), &CratePaths::default()).expect("freight inputs generate");
    assert_eq!(
        files.len(),
        3,
        "one output file per contributing proto file"
    );
    // The freight golden inputs carry no `messages`, so these fixtures stay
    // reflect-free and `roundtrip.rs` can keep compiling them (a `ReflectMessage`
    // impl would need `prost_reflect`, a real pool, and `Self: prost::Message`).
    assert_matches_golden(&files);
}

/// The `ReflectMessage` emission, golden-tested on its own (these fixtures are
/// **not** compiled by `roundtrip.rs`): a top-level message, a two- and
/// three-segment nested message, and a keyword-parent (`Type` -> `r#type`). The
/// pool lookup always breaks across lines here because the standardized `expect`
/// message pushes the chain past `chain_width` — so this also pins that form.
#[test]
fn reflect_messages_match_golden() {
    let messages = vec![
        // Top-level: impl on the bare struct name.
        ReflectMessageName {
            full_name: "reflect.example.v1.Widget".to_owned(),
            path: vec!["Widget".to_owned()],
        },
        // Two-segment nested: parent snake-cased into a module.
        ReflectMessageName {
            full_name: "reflect.example.v1.Outer.Inner".to_owned(),
            path: vec!["Outer".to_owned(), "Inner".to_owned()],
        },
        // Three-segment nested: both parents become modules.
        ReflectMessageName {
            full_name: "reflect.example.v1.Decl.FunctionDecl.Overload".to_owned(),
            path: vec![
                "Decl".to_owned(),
                "FunctionDecl".to_owned(),
                "Overload".to_owned(),
            ],
        },
        // Keyword parent: `Type` is a Rust keyword, so the module is `r#type`.
        ReflectMessageName {
            full_name: "reflect.example.v1.Type.AbstractType".to_owned(),
            path: vec!["Type".to_owned(), "AbstractType".to_owned()],
        },
    ];
    let paths = CratePaths::default().with_descriptor_pool("crate::DESCRIPTOR_POOL".to_owned());

    let files = generate(
        &[GenInput::new(
            "reflect/example/v1/messages.proto".to_owned(),
            vec![],
            vec![],
            messages,
        )],
        &paths,
    )
    .expect("reflect input generates");
    assert_eq!(files.len(), 1, "one reflect-only output file");
    assert_matches_golden(&files);
}

#[test]
fn output_path_swaps_proto_for_aip_rs() {
    let files = generate(&freight_inputs(), &CratePaths::default()).unwrap();
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
    let files = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "example.com/Thing".to_owned(),
                patterns: vec![],
                message_name: Some("Thing".to_owned()),
                has_delete_time: false,
            }],
            vec![],
            vec![],
        )],
        &CratePaths::default(),
    )
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
    let files = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![],
            vec![token_only],
            vec![],
        )],
        &CratePaths::default(),
    )
    .unwrap();
    assert!(files.is_empty(), "half the pagination shape emits nothing");
}

#[test]
fn resource_type_without_a_type_name_is_an_error() {
    let err = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "no-slash".to_owned(),
                patterns: vec!["things/{thing}".to_owned()],
                message_name: Some("Thing".to_owned()),
                has_delete_time: false,
            }],
            vec![],
            vec![],
        )],
        &CratePaths::default(),
    )
    .expect_err("a type with no `service/Type` form cannot name a struct");
    assert!(err.to_string().contains("no-slash"), "{err}");
}

#[test]
fn wildcard_pattern_is_rejected() {
    let err = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "example.com/Thing".to_owned(),
                patterns: vec!["things/-".to_owned()],
                message_name: Some("Thing".to_owned()),
                has_delete_time: false,
            }],
            vec![],
            vec![],
        )],
        &CratePaths::default(),
    )
    .expect_err("a wildcard is not a valid pattern");
    // Surfaced from the runtime `aip_resourcename::Pattern::parse`.
    assert!(err.to_string().contains("wildcard"), "{err}");
}

/// A resource carrying a `delete_time` earns a `SoftDeletable` impl named on its
/// owning prost struct (`message_name`), not the wrapper — emission is
/// resource-anchored (ADR-0014).
#[test]
fn soft_deletable_resource_emits_an_impl_on_its_message() {
    let files = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "example.com/Thing".to_owned(),
                patterns: vec!["things/{thing}".to_owned()],
                message_name: Some("Thing".to_owned()),
                has_delete_time: true,
            }],
            vec![],
            vec![],
        )],
        &CratePaths::default(),
    )
    .unwrap();
    let content = &files[0].content;
    assert!(
        content.contains("impl ::aip_softdelete::SoftDeletable for Thing {"),
        "the impl is named on the message struct, not the wrapper:\n{content}",
    );
    assert!(
        content.contains("::aip_softdelete::State::from_deleted(self.delete_time.is_some())"),
        "the impl reads delete_time presence:\n{content}",
    );
}

/// The negative fixture: a resource WITHOUT a `delete_time` field gets its
/// resource-name wrapper but no `SoftDeletable` impl — a wrong-typed or absent
/// `delete_time` is a silent no-impl, never an error (ADR-0013's near-miss
/// precedent, carried into ADR-0014).
#[test]
fn resource_without_delete_time_emits_no_soft_deletable() {
    let files = generate(
        &[GenInput::new(
            "x/y.proto".to_owned(),
            vec![ResourceDescriptor {
                resource_type: "example.com/Thing".to_owned(),
                patterns: vec!["things/{thing}".to_owned()],
                message_name: Some("Thing".to_owned()),
                has_delete_time: false,
            }],
            vec![],
            vec![],
        )],
        &CratePaths::default(),
    )
    .unwrap();
    let content = &files[0].content;
    // The wrapper is still emitted...
    assert!(content.contains("pub struct ThingResourceName"));
    // ...but no SoftDeletable impl rides along.
    assert!(
        !content.contains("SoftDeletable"),
        "a resource without delete_time must not earn the impl:\n{content}",
    );
}

/// With `aip_crate=aip` the generated code routes references through the
/// umbrella (`::aip::pagination::`, `::aip::resourcename::`, …) instead of
/// the per-crate defaults (`::aip_pagination::`, `::aip_resourcename::`, …).
#[test]
fn aip_crate_option_rewrites_paths() {
    let paths = CratePaths::from_aip_crate(Some("aip"));
    let files =
        generate(&freight_inputs(), &paths).expect("freight inputs generate with aip umbrella");

    // The shipper wrapper should name the pattern static and Error through ::aip::resourcename.
    let shipper = files
        .iter()
        .find(|f| f.path.ends_with("shipper.aip.rs"))
        .expect("shipper.aip.rs generated");
    assert!(
        shipper.content.contains("::aip::resourcename::Pattern"),
        "resourcename path not rewritten:\n{}",
        shipper.content
    );
    assert!(
        shipper.content.contains("::aip::resourcename::Error"),
        "resourcename error path not rewritten:\n{}",
        shipper.content
    );
    assert!(
        !shipper.content.contains("::aip_resourcename::"),
        "old per-crate resourcename path still present:\n{}",
        shipper.content
    );
    // The SoftDeletable impl rides in the same file, routed through ::aip::softdelete.
    assert!(
        shipper
            .content
            .contains("::aip::softdelete::SoftDeletable for Shipper"),
        "softdelete path not rewritten:\n{}",
        shipper.content
    );
    assert!(
        !shipper.content.contains("::aip_softdelete::"),
        "old per-crate softdelete path still present:\n{}",
        shipper.content
    );

    // The freight_service impls should name traits through ::aip::pagination / ::aip::ordering.
    let svc = files
        .iter()
        .find(|f| f.path.ends_with("freight_service.aip.rs"))
        .expect("freight_service.aip.rs generated");
    assert!(
        svc.content.contains("::aip::pagination::PageRequest"),
        "pagination path not rewritten:\n{}",
        svc.content
    );
    assert!(
        svc.content.contains("::aip::ordering::OrderByRequest"),
        "ordering path not rewritten:\n{}",
        svc.content
    );
    assert!(
        !svc.content.contains("::aip_pagination::"),
        "old per-crate pagination path still present:\n{}",
        svc.content
    );
    assert!(
        !svc.content.contains("::aip_ordering::"),
        "old per-crate ordering path still present:\n{}",
        svc.content
    );
}
