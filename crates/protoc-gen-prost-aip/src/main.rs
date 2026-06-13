//! `protoc-gen-prost-aip` — the buf/protoc plugin entry point for aip-rs's
//! resource-name generator, the analog of aip-go's `protoc-gen-go-aip`
//! (ADR-0011).
//!
//! It is a thin shim: read a `CodeGeneratorRequest` from stdin, build a
//! [`DescriptorPool`] over its protos, read the `google.api.resource`
//! annotations with the runtime [`aip_reflect`] helpers, hand them to
//! [`aip_codegen::generate`], and write the resulting files back as a
//! `CodeGeneratorResponse` on stdout. All the generation logic — and its
//! tests — lives in `aip-codegen`; nothing here spawns a process.
//!
//! # Preserving the annotation bytes
//!
//! The annotations we read are protobuf *extensions* (`google.api.resource` is
//! extension 1053 on `MessageOptions`). prost discards unknown/extension fields,
//! so decoding the request's `FileDescriptorProto`s through the generated
//! `prost-types` structs would throw the annotations away (ADR-0011's
//! extension-preservation note). We therefore capture each `proto_file` entry as
//! the *raw bytes* of its length-delimited submessage (a `repeated bytes` field
//! at the same tag) and hand those to prost-reflect's
//! [`decode_file_descriptor_proto`](DescriptorPool::decode_file_descriptor_proto),
//! which preserves extension options.
//!
//! Per the plugin protocol, a failure to *parse* the request is reported on
//! stderr with a non-zero exit (it indicates a problem in protoc itself), while
//! a generation failure is reported in the response's `error` field with a zero
//! exit.

use std::io::{Read as _, Write as _};
use std::process::ExitCode;

use aip_codegen::{generate, CratePaths, GenInput};
use aip_reflect::{
    reflect_messages_in_file, request_descriptors_in_file, resource_descriptors_in_file,
};
use prost::Message as _;
use prost_reflect::DescriptorPool;
use prost_types::compiler::{
    code_generator_response::{Feature, File},
    CodeGeneratorResponse,
};

/// The subset of `google.protobuf.compiler.CodeGeneratorRequest` we need, with
/// `proto_file` captured as raw submessage bytes so extension options survive
/// (see the module docs). On the wire a `repeated bytes` field is identical to a
/// `repeated message` field, so each entry is the verbatim encoded
/// `FileDescriptorProto`.
#[derive(Clone, PartialEq, prost::Message)]
struct CodeGeneratorRequest {
    #[prost(string, repeated, tag = "1")]
    file_to_generate: Vec<String>,
    #[prost(string, optional, tag = "2")]
    parameter: Option<String>,
    #[prost(bytes = "vec", repeated, tag = "15")]
    proto_file: Vec<Vec<u8>>,
}

fn main() -> ExitCode {
    let mut input = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("protoc-gen-prost-aip: failed to read request from stdin: {e}");
        return ExitCode::FAILURE;
    }
    let request = match CodeGeneratorRequest::decode(&input[..]) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("protoc-gen-prost-aip: failed to decode CodeGeneratorRequest: {e}");
            return ExitCode::FAILURE;
        }
    };

    let response = run(request);

    let mut output = Vec::with_capacity(response.encoded_len());
    response
        .encode(&mut output)
        .expect("encoding into a Vec is infallible");
    if let Err(e) = std::io::stdout().write_all(&output) {
        eprintln!("protoc-gen-prost-aip: failed to write response to stdout: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Turn a parsed request into a response, reporting any generation failure in
/// the response's `error` field rather than via the exit code.
fn run(request: CodeGeneratorRequest) -> CodeGeneratorResponse {
    let mut response = CodeGeneratorResponse {
        // The generated `parse`/`format` carries proto3 `optional` fields through
        // unchanged, so we can safely declare support for them.
        supported_features: Some(Feature::Proto3Optional as u64),
        ..Default::default()
    };

    match generate_response(request) {
        Ok(files) => response.file = files,
        Err(error) => response.error = Some(error),
    }
    response
}

/// The plugin's opt-in emission flags, parsed from the `CodeGeneratorRequest`
/// `parameter` (buf's `opt:` entries, comma-joined `key=value` pairs). Bool
/// flags default **off**; an unrecognized key or a value other than
/// `true`/`false` for a bool flag is an error that fails the generation — a
/// typo must not silently disable emission (ADR-0013). The `filtering` flag
/// lands with its emission slice and is unrecognized until then.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Flags {
    /// Emit `impl PageRequest` for pagination-shaped requests.
    pagination: bool,
    /// Emit `impl OrderByRequest` for requests carrying `order_by`.
    ordering: bool,
    /// Emit `impl SoftDeletable` for resource messages carrying `delete_time`
    /// (ADR-0014).
    softdelete: bool,
    /// Route generated crate references through this umbrella root instead of
    /// per-crate names (e.g. `"aip"` -> `::aip::pagination::PageRequest`).
    aip_crate: Option<String>,
    /// Emit `impl ::prost_reflect::ReflectMessage` for every message in each
    /// generated file (ADR-0009). Requires [`reflect_descriptor_pool`].
    ///
    /// [`reflect_descriptor_pool`]: Self::reflect_descriptor_pool
    reflect: bool,
    /// Path to the consumer's `prost_reflect::DescriptorPool` the generated
    /// `ReflectMessage` impls resolve descriptors from (e.g.
    /// `crate::DESCRIPTOR_POOL`). Required when [`reflect`](Self::reflect) is on;
    /// the two in-repo consumers disagree on the path, so there is no default.
    reflect_descriptor_pool: Option<String>,
}

impl Flags {
    /// Parses `parameter` (`None`/`""` means every bool flag off, no crate override).
    fn parse(parameter: Option<&str>) -> Result<Self, String> {
        let mut flags = Self::default();
        for pair in parameter
            .unwrap_or_default()
            .split(',')
            .filter(|p| !p.is_empty())
        {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("invalid parameter {pair:?}: expected `key=value`"))?;
            // `aip_crate` / `reflect_descriptor_pool` take a string value; all
            // other flags take bool. Dispatch the string-valued keys to their
            // `Option<String>` slot in one match, then share the non-empty check.
            let string_slot = match key {
                "aip_crate" => Some(&mut flags.aip_crate),
                "reflect_descriptor_pool" => Some(&mut flags.reflect_descriptor_pool),
                _ => None,
            };
            if let Some(slot) = string_slot {
                if value.is_empty() {
                    return Err(format!(
                        "invalid parameter {pair:?}: `{key}` value must not be empty"
                    ));
                }
                *slot = Some(value.to_owned());
                continue;
            }
            let value = match value {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(format!(
                        "invalid parameter {pair:?}: expected `{key}=true` or `{key}=false`"
                    ))
                }
            };
            match key {
                "pagination" => flags.pagination = value,
                "ordering" => flags.ordering = value,
                "softdelete" => flags.softdelete = value,
                "reflect" => flags.reflect = value,
                _ => return Err(format!("unrecognized parameter key {key:?}")),
            }
        }
        // `reflect` needs a pool to resolve descriptors from; the consumers
        // disagree on the path, so there is no default — a missing one is an
        // error, not a silent wrong guess (ADR-0013's spirit). A pool set without
        // `reflect` is harmless and ignored.
        if flags.reflect && flags.reflect_descriptor_pool.is_none() {
            return Err(
                "`reflect=true` requires `reflect_descriptor_pool=<path to a DescriptorPool>`"
                    .to_owned(),
            );
        }
        Ok(flags)
    }
}

/// Build the descriptor pool, read each requested file's resources and request
/// shapes, and run the generator. Returns the response files or a
/// human-readable error string.
fn generate_response(request: CodeGeneratorRequest) -> Result<Vec<File>, String> {
    let flags = Flags::parse(request.parameter.as_deref())?;
    // protoc/buf send the requested files plus their imports in topological
    // order, so decoding them in turn always satisfies the imports-first rule.
    // Decoding from raw bytes (not `from_file_descriptor_set`) keeps the
    // `google.api.resource` extension options.
    let mut pool = DescriptorPool::new();
    for file in &request.proto_file {
        pool.decode_file_descriptor_proto(&file[..])
            .map_err(|e| format!("building the descriptor pool: {e}"))?;
    }

    // Only the files named on the command line are generated for; their
    // descriptors (and the `google.api.resource` extension definitions) come
    // from the full pool above.
    let mut inputs = Vec::new();
    for name in &request.file_to_generate {
        let Some(file) = pool.get_file_by_name(name) else {
            return Err(format!(
                "file {name:?} is not present in the request's protos"
            ));
        };
        // A disabled flag zeroes the matching presence bools, so the (pure)
        // generator never sees a flag (ADR-0013). `filtering` has no emission
        // yet, so its bool is unconditionally zeroed.
        let mut requests = request_descriptors_in_file(&file);
        for request in &mut requests {
            if !flags.pagination {
                request.has_page_token = false;
                request.has_page_size = false;
                request.has_skip = false;
            }
            if !flags.ordering {
                request.has_order_by = false;
            }
            request.has_filter = false;
        }
        // Same rule for the resource-anchored `SoftDeletable` bool (ADR-0014):
        // `softdelete` off zeroes `has_delete_time` so no impl is emitted.
        let mut resources = resource_descriptors_in_file(&file);
        if !flags.softdelete {
            for resource in &mut resources {
                resource.has_delete_time = false;
            }
        }
        // Reflection is all-or-nothing per message, so there is no presence bool
        // to zero: the plugin simply withholds the message list when `reflect`
        // is off, and the generator emits nothing (ADR-0013's zeroing rule).
        let messages = if flags.reflect {
            reflect_messages_in_file(&file)
        } else {
            Vec::new()
        };
        inputs.push(GenInput::new(name.clone(), resources, requests, messages));
    }

    let mut paths = CratePaths::from_aip_crate(flags.aip_crate.as_deref());
    // `reflect_descriptor_pool` is orthogonal to the umbrella root, so it is set
    // here rather than through `from_aip_crate`. Parsing guarantees it is present
    // whenever `reflect` populated any `messages` above.
    paths.descriptor_pool = flags.reflect_descriptor_pool.clone();
    let files = generate(&inputs, &paths).map_err(|e| e.to_string())?;
    Ok(files
        .into_iter()
        .map(|f| File {
            name: Some(f.path),
            content: Some(f.content),
            ..Default::default()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::Flags;

    #[test]
    fn flags_default_off() {
        assert_eq!(Flags::parse(None).unwrap(), Flags::default());
        assert_eq!(Flags::parse(Some("")).unwrap(), Flags::default());
        assert_eq!(
            Flags::default(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
    }

    #[test]
    fn pagination_flag_parses() {
        assert_eq!(
            Flags::parse(Some("pagination=true")).unwrap(),
            Flags {
                pagination: true,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
        assert_eq!(
            Flags::parse(Some("pagination=false")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
    }

    #[test]
    fn ordering_flag_parses() {
        assert_eq!(
            Flags::parse(Some("ordering=true")).unwrap(),
            Flags {
                pagination: false,
                ordering: true,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
        assert_eq!(
            Flags::parse(Some("ordering=false")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
    }

    #[test]
    fn softdelete_flag_parses() {
        assert_eq!(
            Flags::parse(Some("softdelete=true")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: true,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
        assert_eq!(
            Flags::parse(Some("softdelete=false")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
    }

    /// Each flag is parsed independently; commas join them.
    #[test]
    fn flags_combine() {
        assert_eq!(
            Flags::parse(Some("pagination=true,ordering=true,softdelete=true")).unwrap(),
            Flags {
                pagination: true,
                ordering: true,
                softdelete: true,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
    }

    #[test]
    fn aip_crate_flag_parses() {
        assert_eq!(
            Flags::parse(Some("aip_crate=aip")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: Some("aip".to_owned()),
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
        // Combines with bool flags.
        assert_eq!(
            Flags::parse(Some("pagination=true,aip_crate=aip,ordering=true")).unwrap(),
            Flags {
                pagination: true,
                ordering: true,
                softdelete: false,
                aip_crate: Some("aip".to_owned()),
                reflect: false,
                reflect_descriptor_pool: None,
            }
        );
        // Empty value is an error.
        assert!(Flags::parse(Some("aip_crate=")).is_err());
    }

    /// `reflect` needs a pool path: on with one parses, on without one errors,
    /// and an empty path errors (like `aip_crate`).
    #[test]
    fn reflect_flag_parses_and_requires_a_pool() {
        assert_eq!(
            Flags::parse(Some(
                "reflect=true,reflect_descriptor_pool=crate::DESCRIPTOR_POOL"
            ))
            .unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: true,
                reflect_descriptor_pool: Some("crate::DESCRIPTOR_POOL".to_owned()),
            }
        );
        // On without a pool is an error — no silent wrong default.
        assert!(Flags::parse(Some("reflect=true")).is_err());
        // Empty pool value is an error.
        assert!(Flags::parse(Some("reflect=true,reflect_descriptor_pool=")).is_err());
    }

    /// A pool path set while `reflect` is off is harmless and ignored, not an
    /// error — it is redundant config, not a typo.
    #[test]
    fn reflect_pool_without_reflect_is_ignored() {
        assert_eq!(
            Flags::parse(Some("reflect_descriptor_pool=crate::DESCRIPTOR_POOL")).unwrap(),
            Flags {
                pagination: false,
                ordering: false,
                softdelete: false,
                aip_crate: None,
                reflect: false,
                reflect_descriptor_pool: Some("crate::DESCRIPTOR_POOL".to_owned()),
            }
        );
    }

    /// A typo must fail the generation, not silently disable emission.
    #[test]
    fn unrecognized_key_or_value_is_an_error() {
        assert!(Flags::parse(Some("paginatoin=true")).is_err());
        assert!(Flags::parse(Some("pagination=yes")).is_err());
        assert!(Flags::parse(Some("pagination")).is_err());
        assert!(Flags::parse(Some("ordering=yes")).is_err());
        assert!(Flags::parse(Some("softdelete=yes")).is_err());
        assert!(Flags::parse(Some("reflect=yes")).is_err());
        // `filtering` is unrecognized until its slice lands.
        assert!(Flags::parse(Some("filtering=true")).is_err());
    }
}
