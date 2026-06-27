//! The generation logic: walk `google.api.resource` descriptors and emit typed
//! resource-name wrappers layered on the runtime [`aip_resourcename::Pattern`]
//! API (ADR-0011) plus an `impl aip_softdelete::SoftDeletable` on each resource
//! message carrying a `delete_time` (ADR-0014), and walk **Request descriptors**
//! and emit `impl aip_pagination::PageRequest` /
//! `impl aip_ordering::OrderByRequest` keyed on field shape (ADR-0013), and —
//! when reflection is on — an `impl ::prost_reflect::ReflectMessage` for *every*
//! message of the file (nested included, map-entries excluded), resolving each
//! one's descriptor from the consumer's pool (ADR-0009).
//!
//! Generated files are `use`-free and fully path-qualified
//! (`::aip_resourcename::…`, `::aip_pagination::…`), so the consumer mounts
//! every `*.aip.rs` directly in the module holding the prost message structs —
//! one mount rule whether the file carries wrappers, trait impls, or both
//! (ADR-0013). A trait impl names its prost struct (and a wrapper's [`parent`]
//! its parent wrapper) by bare path in that shared module.
//!
//! Each resource yields a `<Type>ResourceName` struct — one **private** `String`
//! field per pattern variable, plus the canonical resource name formatted once at
//! construction — built through a validated `new(...) -> Result<Self, Error>`
//! constructor (each variable checked with [`aip_resourcename::validate_variable`]).
//! Every constructor funnels through one private `from_parts` that formats and
//! stores that name, so `as_str(&self) -> &str` and `AsRef<str>` hand it back
//! with no allocation. Because construction validates every variable, the wrapper
//! formats infallibly: it implements [`Display`] (writing the stored name), plus
//! [`FromStr`] (delegating to `parse`), the matching `TryFrom<&str>` /
//! `TryFrom<String>` (same `Error`, for `.try_into()` and generic bounds), and
//! `From<Self> for String` (moving out the stored name). The stored name is
//! redundant with the variables, so `Eq` / `Hash` are unchanged. The wrapper also
//! implements `Ord` / `PartialOrd` **over that stored name** — string order, the
//! same a `BTreeMap<String, _>` or SQL `ORDER BY name` gives — not a field-tuple
//! derive (the two diverge when one variable value is a prefix of another). The compiled
//! [`Pattern`] is built once per wrapper in a `LazyLock`, shared by `parse` and
//! `Display`. The generator does **not** reimplement the runtime; the emitted
//! code calls into it.
//!
//! A multi-segment wrapper also gets a [`parent`] accessor returning the parent
//! pattern's typed wrapper when that parent pattern is generated in the same
//! invocation.
//!
//! A resource message also earns an `impl SoftDeletable` — reading its
//! `delete_time` presence as the AIP-164 soft-delete state — iff it carries a
//! `google.protobuf.Timestamp delete_time` field (resource-anchored emission,
//! ADR-0014). The bool arrives pre-zeroed by the plugin when the `softdelete`
//! flag is off, so this generator never sees a flag. The impl is independent of
//! the wrapper: a patternless soft-deletable resource still earns it.
//!
//! A request message qualifies for `PageRequest` iff it has plain `string
//! page_token` **and** `int32 page_size` fields — the Rust analog of aip-go's
//! structural interface satisfaction — with a `skip()` override added only when
//! it also has `int32 skip` (otherwise the trait's `0` default stands). It
//! qualifies for `OrderByRequest` iff it has a plain `string order_by` field.
//! The two emissions are independent (a request can earn either, both, or
//! neither). The presence bools arrive pre-zeroed by the plugin when the
//! matching `pagination` / `ordering` flag is off, so this generator never sees
//! a flag.
//!
//! A multi-pattern resource is emitted as the single-pattern wrapper of its
//! first pattern (aip-go's `SinglePatternStructName`); the multi-pattern
//! interface variants are deferred (issue #62).
//!
//! Pattern variables become struct field names verbatim, which assumes they are
//! valid snake_case Rust identifiers — true for the AIP-122 lower-case resource
//! ids in scope (`{shipper}`, `{site}`). Escaping a variable that is a Rust
//! keyword or `lowerCamelCase` is deferred with the rest of the codegen (#62 /
//! #82); no such resource exists in the example yet.
//!
//! [`Display`]: std::fmt::Display
//! [`FromStr`]: std::str::FromStr
//! [`parent`]: https://google.aip.dev/122

use std::collections::BTreeMap;
use std::fmt::Write as _;

use aip_reflect::{ReflectMessageName, RequestDescriptor, ResourceDescriptor, ResourceType};
use aip_resourcename::{Pattern, Scanner};
use heck::ToSnakeCase as _;

use crate::Error;

/// Crate-root paths used in generated code, so the consumer can route
/// references through the `aip` umbrella (`aip_crate=aip` -> `::aip::pagination::…`)
/// instead of the per-crate names (`::aip_pagination::…`).
///
/// Construct with [`CratePaths::default`] for per-crate paths, or
/// [`CratePaths::from_aip_crate`] with an umbrella root like `"aip"`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CratePaths {
    /// Module path to `aip-resourcename`, e.g. `::aip_resourcename` or `::aip::resourcename`.
    pub resourcename: String,
    /// Module path to `aip-resourceid`, e.g. `::aip_resourceid` or `::aip::resourceid`.
    /// Used by the generated `mint` / `mint_under` constructors.
    pub resourceid: String,
    /// Module path to `aip-pagination`, e.g. `::aip_pagination` or `::aip::pagination`.
    pub pagination: String,
    /// Module path to `aip-ordering`, e.g. `::aip_ordering` or `::aip::ordering`.
    pub ordering: String,
    /// Module path to `aip-softdelete`, e.g. `::aip_softdelete` or `::aip::softdelete`.
    /// Used by the generated `SoftDeletable` impls.
    pub softdelete: String,
    /// Path to the consumer's `prost_reflect::DescriptorPool`, e.g.
    /// `crate::DESCRIPTOR_POOL` or `crate::proto::DESCRIPTOR_POOL`, that the
    /// generated `ReflectMessage` impls resolve each message's descriptor from
    /// (ADR-0009). `None` when reflection is off; required (non-`None`) whenever a
    /// [`GenInput`] carries [`messages`](GenInput::messages), an invariant the
    /// plugin enforces. Not a crate root, so [`from_aip_crate`](Self::from_aip_crate)
    /// leaves it `None` for the caller to set.
    pub descriptor_pool: Option<String>,
}

impl Default for CratePaths {
    fn default() -> Self {
        Self {
            resourcename: "::aip_resourcename".to_owned(),
            resourceid: "::aip_resourceid".to_owned(),
            pagination: "::aip_pagination".to_owned(),
            ordering: "::aip_ordering".to_owned(),
            softdelete: "::aip_softdelete".to_owned(),
            descriptor_pool: None,
        }
    }
}

impl CratePaths {
    /// Set the [`descriptor_pool`](Self::descriptor_pool) path, consuming and
    /// returning `self`. Pairs with [`default`](Self::default) or
    /// [`from_aip_crate`](Self::from_aip_crate) for the common case:
    /// `CratePaths::default().with_descriptor_pool("crate::POOL".to_owned())`.
    pub fn with_descriptor_pool(mut self, pool: String) -> Self {
        self.descriptor_pool = Some(pool);
        self
    }

    /// Build paths rooted at `aip_crate` (e.g. `"aip"` -> `::aip::pagination::…`).
    /// `None` returns the per-crate default.
    pub fn from_aip_crate(aip_crate: Option<&str>) -> Self {
        match aip_crate {
            None => Self::default(),
            Some(root) => Self {
                resourcename: format!("::{root}::resourcename"),
                resourceid: format!("::{root}::resourceid"),
                pagination: format!("::{root}::pagination"),
                ordering: format!("::{root}::ordering"),
                softdelete: format!("::{root}::softdelete"),
                // Orthogonal to the umbrella root and consumer-specific; the
                // caller sets it from the `reflect_descriptor_pool` opt.
                descriptor_pool: None,
            },
        }
    }
}

/// One generated source file: a relative output [`path`](Self::path) and its
/// Rust [`content`](Self::content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenFile {
    /// The output path, relative to the plugin's output directory, e.g.
    /// `einride/example/freight/v1/shipper.aip.rs`.
    pub path: String,
    /// The generated Rust source.
    pub content: String,
}

/// The resources and request messages declared in one proto file — the unit of
/// generation.
///
/// The plugin builds these from a `CodeGeneratorRequest` (via runtime
/// `aip-reflect`); a golden test constructs them directly.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GenInput {
    /// The source proto file path, e.g.
    /// `einride/example/freight/v1/shipper.proto`. Determines the output path.
    pub proto_file: String,
    /// The `google.api.resource` descriptors declared in that file.
    pub resources: Vec<ResourceDescriptor>,
    /// The request descriptors of that file's top-level messages, with each
    /// presence bool already zeroed by the plugin when its flag is off
    /// (ADR-0013).
    pub requests: Vec<RequestDescriptor>,
    /// Every message in the file (nested included, map-entries excluded) that
    /// earns a `prost_reflect::ReflectMessage` impl (ADR-0009). The plugin
    /// populates this only when its `reflect` flag is on (empty otherwise), so
    /// the generator never sees a flag — the same zeroing rule the other
    /// emissions use (ADR-0013). A non-empty list requires
    /// [`CratePaths::descriptor_pool`] to be set.
    pub messages: Vec<ReflectMessageName>,
}

impl GenInput {
    /// Construct a `GenInput`. Required because the struct is `#[non_exhaustive]`.
    pub fn new(
        proto_file: String,
        resources: Vec<ResourceDescriptor>,
        requests: Vec<RequestDescriptor>,
        messages: Vec<ReflectMessageName>,
    ) -> Self {
        Self {
            proto_file,
            resources,
            requests,
            messages,
        }
    }
}

/// Generate typed resource-name wrappers, request-trait impls, and
/// `ReflectMessage` impls, one output [`GenFile`] per input file whose body
/// carries at least one of them.
///
/// A resource with no patterns contributes nothing, as does a request without
/// the pagination field shape; with reflection off a file carries only its
/// wrappers/request-impls, so a file contributing none of the three produces no
/// file at all (matching aip-go).
pub fn generate(inputs: &[GenInput], paths: &CratePaths) -> Result<Vec<GenFile>, Error> {
    if inputs.iter().any(|i| !i.messages.is_empty()) && paths.descriptor_pool.is_none() {
        return Err(Error::MissingDescriptorPool);
    }

    // A `pattern -> <Type>ResourceName` index over every resource in the whole
    // invocation, so a multi-segment wrapper can resolve its parent pattern to
    // the parent's typed wrapper even when that parent lives in another file.
    let wrappers_by_pattern = index_wrappers(inputs)?;

    let mut files = Vec::new();
    for input in inputs {
        if let Some(content) = generate_file(
            &input.resources,
            &input.requests,
            &input.messages,
            &wrappers_by_pattern,
            paths,
        )? {
            files.push(GenFile {
                path: output_path(&input.proto_file),
                content,
            });
        }
    }
    Ok(files)
}

/// Build the `pattern -> struct name` index of every patterned resource across
/// all inputs (each resource's first pattern, matching what is generated).
fn index_wrappers(inputs: &[GenInput]) -> Result<BTreeMap<String, String>, Error> {
    let mut by_pattern = BTreeMap::new();
    for input in inputs {
        for resource in &input.resources {
            if let Some(pattern) = resource.patterns.first() {
                by_pattern.insert(pattern.clone(), struct_name(&resource.resource_type)?);
            }
        }
    }
    Ok(by_pattern)
}

/// The output path for a generated file: the proto path with a trailing
/// `.proto` replaced by `.aip.rs` (else `.aip.rs` appended).
fn output_path(proto_file: &str) -> String {
    let stem = proto_file.strip_suffix(".proto").unwrap_or(proto_file);
    format!("{stem}.aip.rs")
}

/// Generate the body of one file's resource-name wrappers, request-trait impls,
/// and `ReflectMessage` impls, or `None` if it would be empty.
///
/// `wrappers_by_pattern` is the whole-invocation `pattern -> struct name` index
/// used to resolve a multi-segment wrapper's [`parent`](https://google.aip.dev/122).
fn generate_file(
    resources: &[ResourceDescriptor],
    requests: &[RequestDescriptor],
    messages: &[ReflectMessageName],
    wrappers_by_pattern: &BTreeMap<String, String>,
    paths: &CratePaths,
) -> Result<Option<String>, Error> {
    let mut body = String::new();
    for resource in resources {
        if let Some(pattern) = resource.patterns.first() {
            write_resource_name(
                &mut body,
                &resource.resource_type,
                pattern,
                wrappers_by_pattern,
                paths,
            )?;
        }
        // The `SoftDeletable` impl is resource-anchored but pattern-independent:
        // it keys on the message carrying a `delete_time`, named by its prost
        // struct (ADR-0014). The bool arrives pre-zeroed when the plugin's
        // `softdelete` flag is off, so this generator never sees a flag.
        if resource.has_delete_time {
            if let Some(message_name) = &resource.message_name {
                write_soft_deletable(&mut body, message_name, paths);
            }
        }
    }
    for request in requests {
        if request.has_page_token && request.has_page_size {
            write_page_request(&mut body, request, paths);
        }
        if request.has_order_by {
            write_order_by_request(&mut body, request, paths);
        }
    }
    // The `ReflectMessage` impls come last and cover *every* message (ADR-0009),
    // so a message that is also a resource/request earns this in addition to its
    // wrapper/trait impls. The list is empty when the plugin's `reflect` flag is
    // off, so this loop simply does nothing then.
    for message in messages {
        write_reflect_message(&mut body, message, paths);
    }
    if body.is_empty() {
        return Ok(None);
    }
    let mut out = String::new();
    out.push_str("// @generated by protoc-gen-prost-aip (aip-codegen). DO NOT EDIT.\n");
    out.push_str(&body);
    Ok(Some(out))
}

/// Append the `PageRequest` impl for `request`, overriding the trait's `skip()`
/// default only when the message has a `skip` field. The prost struct is named
/// by bare path — the impl lands in the module that holds the generated message
/// structs (ADR-0013's mount rule).
fn write_page_request(out: &mut String, request: &RequestDescriptor, paths: &CratePaths) {
    let mut line = |s: &str| {
        let _ = writeln!(out, "{s}");
    };
    let message_name = &request.message_name;
    let pagination = &paths.pagination;

    line("");
    line(&format!(
        "/// AIP-158 pagination accessors, generated from `{message_name}`'s field shape."
    ));
    line(&format!(
        "impl {pagination}::PageRequest for {message_name} {{"
    ));
    line("    fn page_token(&self) -> &str {");
    line("        &self.page_token");
    line("    }");
    line("");
    line("    fn page_size(&self) -> i32 {");
    line("        self.page_size");
    line("    }");
    if request.has_skip {
        line("");
        line("    fn skip(&self) -> i32 {");
        line("        self.skip");
        line("    }");
    }
    line("}");
}

/// Append the `OrderByRequest` impl for `request`, reading its plain `string
/// order_by` field. Named by bare path, landing in the module that holds the
/// generated message structs (ADR-0013's mount rule).
fn write_order_by_request(out: &mut String, request: &RequestDescriptor, paths: &CratePaths) {
    let mut line = |s: &str| {
        let _ = writeln!(out, "{s}");
    };
    let message_name = &request.message_name;
    let ordering = &paths.ordering;

    line("");
    line(&format!(
        "/// AIP-132 ordering accessor, generated from `{message_name}`'s field shape."
    ));
    line(&format!(
        "impl {ordering}::OrderByRequest for {message_name} {{"
    ));
    line("    fn order_by(&self) -> &str {");
    line("        &self.order_by");
    line("    }");
    line("}");
}

/// Append the `SoftDeletable` impl for `message_name`, reading the resource's
/// `delete_time` presence as its AIP-164 soft-delete state. The prost struct is
/// named by bare path — the impl lands in the module that holds the generated
/// message structs (ADR-0013's mount rule, shared by ADR-0014).
fn write_soft_deletable(out: &mut String, message_name: &str, paths: &CratePaths) {
    let mut line = |s: &str| {
        let _ = writeln!(out, "{s}");
    };
    let softdelete = &paths.softdelete;

    line("");
    line(&format!(
        "/// AIP-164 soft-delete state, generated from `{message_name}`'s `delete_time` field."
    ));
    line(&format!(
        "impl {softdelete}::SoftDeletable for {message_name} {{"
    ));
    line(&format!(
        "    fn soft_delete_state(&self) -> {softdelete}::State {{"
    ));
    line(&format!(
        "        {softdelete}::State::from_deleted(self.delete_time.is_some())"
    ));
    line("    }");
    line("}");
}

/// Append the `prost_reflect::ReflectMessage` impl for `message`, resolving its
/// descriptor from the consumer's pool by proto name (ADR-0009). The prost
/// struct is named by Rust path — bare for a top-level message, module-qualified
/// for a nested one — landing in the module that holds the generated structs
/// (ADR-0013's mount rule). `::prost_reflect` is hard-coded: it is a third-party
/// crate the consumer already depends on, not one routed through the `aip`
/// umbrella.
fn write_reflect_message(out: &mut String, message: &ReflectMessageName, paths: &CratePaths) {
    let mut line = |s: &str| {
        let _ = writeln!(out, "{s}");
    };
    // Invariant: a non-empty `messages` list means the plugin set the pool path
    // (it errors otherwise). The generator never has to handle the missing case.
    let pool = paths
        .descriptor_pool
        .as_deref()
        .expect("descriptor_pool is set whenever reflect messages are emitted");
    let rust_path = rust_path(&message.path);
    let fqn = &message.full_name;

    line("");
    line("/// Reflection wiring (ADR-0009): resolves this message's descriptor from the pool.");
    line(&format!(
        "impl ::prost_reflect::ReflectMessage for {rust_path} {{"
    ));
    line("    fn descriptor(&self) -> ::prost_reflect::MessageDescriptor {");
    line(&format!("        {pool}"));
    line(&format!("            .get_message_by_name(\"{fqn}\")"));
    line(&format!(
        "            .expect(\"descriptor pool contains {fqn}\")"
    ));
    line("    }");
    line("}");
}

/// The Rust path naming `path`'s prost struct, within the module the generated
/// file is mounted in: the leaf message name verbatim, each parent message name
/// snake-cased into its prost module (`["Decl", "FunctionDecl", "Overload"]` ->
/// `decl::function_decl::Overload`; `["Shipper"]` -> `Shipper`).
fn rust_path(path: &[String]) -> String {
    let (leaf, parents) = path
        .split_last()
        .expect("a message path holds at least its own name");
    let mut segments: Vec<String> = parents.iter().map(|p| snake_module(p)).collect();
    segments.push(leaf.clone());
    segments.join("::")
}

/// Message name -> the prost module name nesting it: `heck::ToSnakeCase` (matching
/// prost-build's own `to_snake` exactly, including acronym handling: `XMLHttpRequest`
/// -> `xml_http_request`), then prost-build's keyword sanitization.
fn snake_module(name: &str) -> String {
    sanitize_keyword(name.to_snake_case())
}

/// prost-build's identifier keyword handling, ported verbatim from prost-build
/// 0.14's `sanitize_identifier` (the version `neoeinstein-prost:v0.5.0` builds
/// on) so a generated module name matches the one prost emits: a Rust keyword
/// that can be a raw identifier becomes `r#kw` (so `type` -> `r#type`, `gen` ->
/// `r#gen`); the ones that cannot — `self`, `super`, `extern`, `crate`, `Self`,
/// `_` — take a trailing `_`; a numeric-leading name is prefixed with `_`. The
/// caller has already snake-cased (lowercased) the name, so the `Self` and
/// numeric arms never fire on a real proto identifier — they are kept only so
/// this stays a faithful copy that won't drift from prost.
fn sanitize_keyword(mut ident: String) -> String {
    match ident.as_str() {
        // 2015 strict keywords.
        "as" | "break" | "const" | "continue" | "else" | "enum" | "false" | "fn" | "for" | "if"
        | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub" | "ref"
        | "return" | "static" | "struct" | "trait" | "true" | "type" | "unsafe" | "use"
        | "where" | "while"
        // 2018 strict keywords.
        | "dyn"
        // 2015 reserved keywords.
        | "abstract" | "become" | "box" | "do" | "final" | "macro" | "override" | "priv"
        | "typeof" | "unsized" | "virtual" | "yield"
        // 2018 reserved keywords.
        | "async" | "await" | "try"
        // 2024 reserved keywords.
        | "gen" => ident.insert_str(0, "r#"),
        // These keywords cannot be raw identifiers, so prost suffixes them.
        "_" | "self" | "super" | "Self" | "extern" | "crate" => ident.push('_'),
        // A numeric-leading identifier is invalid, so prost prefixes it.
        _ if ident.starts_with(|c: char| c.is_numeric()) => ident.insert(0, '_'),
        _ => {}
    }
    ident
}

/// The `<Type>ResourceName` struct name for `resource_type`, or an error if the
/// type has no `service/Type` form to take the type name from.
fn struct_name(resource_type: &str) -> Result<String, Error> {
    let resource = ResourceType::new(resource_type);
    let type_name = resource.type_name();
    if type_name.is_empty() {
        return Err(Error::InvalidResource {
            resource_type: resource_type.to_owned(),
            reason: "resource type has no type name (expected `service/Type`)".to_owned(),
        });
    }
    Ok(format!("{type_name}ResourceName"))
}

/// Append one `<Type>ResourceName` wrapper for `resource_type` over `pattern`.
fn write_resource_name(
    out: &mut String,
    resource_type: &str,
    pattern: &str,
    wrappers_by_pattern: &BTreeMap<String, String>,
    paths: &CratePaths,
) -> Result<(), Error> {
    let rn = &paths.resourcename;
    let struct_name = struct_name(resource_type)?;
    let variables = pattern_variables(pattern)?;
    // The `LazyLock<Pattern>` holding the compiled pattern, named off the struct.
    let pattern_const = format!("{}_PATTERN", screaming_snake(&struct_name));

    // The parent wrapper, if this pattern's parent pattern is itself generated in
    // this invocation: its `(pattern, struct name, variables)`. The variables are
    // the leading subset of `variables` the parent's `new` consumes.
    let parent = match parent_pattern(pattern) {
        Some(pp) => match wrappers_by_pattern.get(&pp) {
            Some(name) => Some((pp.clone(), name.clone(), pattern_variables(&pp)?)),
            None => None,
        },
        None => None,
    };

    // Each `writeln!` is one line of generated source; the leading spaces are
    // the indentation of the emitted code, not of this source. `out` is a
    // `String`, so the writes never fail.
    let mut line = |s: &str| {
        let _ = writeln!(out, "{s}");
    };

    line("");
    line(&format!("/// Typed resource name for `{resource_type}`."));
    line("///");
    line(&format!(
        "/// Generated from the `google.api.resource` pattern `{pattern}`."
    ));
    line("#[derive(Clone, Debug, PartialEq, Eq, Hash)]");
    line(&format!("pub struct {struct_name} {{"));
    for var in &variables {
        line(&format!("    {var}: String,"));
    }
    // The canonical resource name, formatted once at construction and stored so
    // `as_str` / `Display` hand back a `&str` with no per-call Pattern re-format.
    // Redundant with the variables (they determine it 1:1), so it does not change
    // `Eq` / `Hash`: two wrappers are equal iff their variables are.
    line("    /// canonical resource name. built once. backs as_str + Display.");
    line("    name: String,");
    line("}");
    line("");
    line(&format!(
        "/// The compiled `{pattern}` pattern, parsed once."
    ));
    // The fully-qualified type overflows the one-line form for any plausible
    // struct name, so the static is emitted pre-broken the way rustfmt breaks
    // it: after the `=`, then the `.expect` under the `parse` call.
    line(&format!(
        "static {pattern_const}: ::std::sync::LazyLock<{rn}::Pattern> ="
    ));
    line("    ::std::sync::LazyLock::new(|| {");
    line(&format!(
        "        {rn}::Pattern::parse({struct_name}::PATTERN)"
    ));
    line("            .expect(\"a generated pattern parses\")");
    line("    });");
    line("");
    line(&format!("impl {struct_name} {{"));
    line(&format!("    /// The resource type, `{resource_type}`."));
    line(&format!(
        "    pub const TYPE: &'static str = \"{resource_type}\";"
    ));
    line("");
    line(&format!("    /// The resource name pattern, `{pattern}`."));
    line(&format!(
        "    pub const PATTERN: &'static str = \"{pattern}\";"
    ));
    line("");

    // `from_parts` — the one place the canonical name is formatted. Every public
    // constructor funnels its already-valid variables through here, so the name
    // is built exactly once (per construction) and the format logic lives once.
    // The format-array (`[(var, value), …]`) collapses to one line under the
    // width limit, else one pair per line, matching how rustfmt would break it.
    let parts_pairs: Vec<String> = variables
        .iter()
        .map(|var| format!("(\"{var}\", {var}.as_str())"))
        .collect();
    let parts_params: Vec<String> = variables
        .iter()
        .map(|var| format!("{var}: String"))
        .collect();
    line("    /// Build from already-validated variables, formatting the canonical");
    line("    /// resource name once and storing it. Private: callers go through the");
    line("    /// validating/parsing constructors that guarantee the variables hold.");
    let parts_sig = format!("    fn from_parts({}) -> Self {{", parts_params.join(", "));
    if parts_sig.len() <= RUSTFMT_MAX_WIDTH {
        line(&parts_sig);
    } else {
        line("    fn from_parts(");
        for param in &parts_params {
            line(&format!("        {param},"));
        }
        line("    ) -> Self {");
    }
    let parts_one_line = format!(
        "        let name = {pattern_const}.format([{}]).expect(\"a validated resource name formats\");",
        parts_pairs.join(", ")
    );
    if parts_one_line.len() <= RUSTFMT_MAX_WIDTH {
        line(&parts_one_line);
    } else {
        line(&format!("        let name = {pattern_const}"));
        let array = format!("[{}]", parts_pairs.join(", "));
        if array.len() <= RUSTFMT_ARRAY_WIDTH {
            line(&format!("            .format({array})"));
        } else {
            line("            .format([");
            for pair in &parts_pairs {
                line(&format!("                {pair},"));
            }
            line("            ])");
        }
        line("            .expect(\"a validated resource name formats\");");
    }
    let mut parts_fields = variables.clone();
    parts_fields.push("name".to_owned());
    line(&format!("        Self {{ {} }}", parts_fields.join(", ")));
    line("    }");
    line("");

    // `new` — validated construction over `impl Into<String>` variables. The
    // signature collapses onto one line when it fits the 100-column limit,
    // otherwise one parameter per line, matching rustfmt.
    line("    /// Construct the resource name from its variables, validating each");
    line("    /// as a single resource-name segment (non-empty, no `/`).");
    let params: Vec<String> = variables
        .iter()
        .map(|var| format!("{var}: impl Into<String>"))
        .collect();
    let inline_sig = format!(
        "    pub fn new({}) -> Result<Self, {rn}::Error> {{",
        params.join(", ")
    );
    if inline_sig.len() <= RUSTFMT_MAX_WIDTH {
        line(&inline_sig);
    } else {
        line("    pub fn new(");
        for param in &params {
            line(&format!("        {param},"));
        }
        line(&format!("    ) -> Result<Self, {rn}::Error> {{"));
    }
    for var in &variables {
        line(&format!("        let {var} = {var}.into();"));
    }
    for var in &variables {
        line(&format!(
            "        {rn}::validate_variable(\"{var}\", &{var})?;"
        ));
    }
    line(&format!(
        "        Ok(Self::from_parts({}))",
        variables.join(", ")
    ));
    line("    }");

    for var in &variables {
        line("");
        line(&format!("    /// The `{{{var}}}` variable."));
        line(&format!("    pub fn {var}(&self) -> &str {{"));
        line(&format!("        &self.{var}"));
        line("    }");
    }

    // `as_str` — the canonical resource name as a borrowed `&str`, no allocation
    // (it was formatted once at construction). Call sites hold the typed wrapper
    // and address storage through this instead of falling back to raw `String`.
    line("");
    line("    /// The canonical resource name as a string slice — no allocation.");
    line("    pub fn as_str(&self) -> &str {");
    line("        &self.name");
    line("    }");

    line("");
    line("    /// Parse a resource name string into its typed variables.");
    line(&format!(
        "    pub fn parse(name: &str) -> Result<Self, {rn}::Error> {{"
    ));
    line(&format!(
        "        let Some(captures) = {pattern_const}.match_name(name) else {{"
    ));
    line(&format!(
        "            return Err({rn}::Error::PatternMismatch {{"
    ));
    line("                pattern: Self::PATTERN.to_owned(),");
    line("            });");
    line("        };");
    for var in &variables {
        line(&format!("        let {var} = captures"));
        line(&format!("            .get(\"{var}\")"));
        line(&format!(
            "            .ok_or_else(|| {rn}::Error::MissingVariable {{"
        ));
        line(&format!("                name: \"{var}\".to_owned(),"));
        line("            })?");
        line("            .to_owned();");
    }
    line(&format!(
        "        Ok(Self::from_parts({}))",
        variables.join(", ")
    ));
    line("    }");

    // `parse_field` — validates the value as a *concrete* resource name (no `-`
    // wildcard, via `validate_strict`) then matches the pattern, wrapping every
    // error with the request field path as a `FieldError`. A `Get`/mutation name
    // must address one resource, so a wildcard is rejected here as
    // `INVALID_ARGUMENT` rather than falling through to `NOT_FOUND` (AIP-159);
    // the `List`-parent counterpart that *accepts* `-` is `parse_parent_field`.
    // Handlers parse once and propagate via `?` to get the right AIP-193
    // `BadRequest` field violation.
    line("");
    line("    /// Parse `value` from request field `field` as a concrete resource");
    line("    /// name, wrapping any error with the field path so `?` produces an");
    line("    /// AIP-193 `BadRequest` violation. Rejects a `-` wildcard segment;");
    line("    /// use `parse_parent_field` for a `List` parent that may carry one.");
    line(&format!(
        "    pub fn parse_field(field: &str, value: &str) -> Result<Self, {rn}::FieldError> {{"
    ));
    line("        let wrap = |source| {");
    line(&format!(
        "            {rn}::FieldError {{ field: field.to_owned(), source }}"
    ));
    line("        };");
    line(&format!(
        "        {rn}::validate_strict(value).map_err(wrap)?;"
    ));
    line("        Self::parse(value).map_err(wrap)");
    line("    }");

    // `parse_parent_field` — the AIP-159 `List`-parent counterpart to
    // `parse_field`: it accepts a `-` wildcard in any resource-id position
    // (`shippers/-` to list across every shipper) while the collection-id
    // segments must still match this pattern. It returns a borrowed
    // `ParentName` — a view that may carry wildcards, never a concrete
    // `<Type>ResourceName` — so the wildcard can't masquerade as a real
    // resource; the handler feeds `as_str()` to the SQL parent scope and walks
    // `segments()` to authorize any wildcard position. The same `FieldError`
    // wrapping as `parse_field`, so a bare `?` yields the right AIP-193 violation.
    line("");
    line("    /// Parse `value` from request field `field` as a `List` parent that may");
    line("    /// carry `-` wildcard segments (AIP-159), wrapping any error with the");
    line("    /// field path for an AIP-193 `BadRequest`. A wildcard is accepted in any");
    line("    /// resource-id position; the collection-id segments must match this");
    line("    /// pattern. Returns a borrowed `ParentName` view.");
    line("    pub fn parse_parent_field<'a>(");
    line("        field: &str,");
    line("        value: &'a str,");
    line(&format!(
        "    ) -> Result<{rn}::ParentName<'a>, {rn}::FieldError> {{"
    ));
    line("        let wrap = |source| {");
    line(&format!(
        "            {rn}::FieldError {{ field: field.to_owned(), source }}"
    ));
    line("        };");
    line(&format!("        {rn}::validate(value).map_err(wrap)?;"));
    line(&format!(
        "        {pattern_const}.match_parent(value).ok_or_else(|| {{"
    ));
    line(&format!(
        "            wrap({rn}::Error::PatternMismatch {{ pattern: Self::PATTERN.to_owned() }})"
    ));
    line("        })");
    line("    }");

    // `mint` — infallible constructor for single-variable (no parent) wrappers
    // only; `mint_under` is the parented variant.
    if parent.is_none() && variables.len() == 1 {
        let ri = &paths.resourceid;
        line("");
        line("    /// Mint a resource name with a system-assigned ID (AIP-148).");
        line("    /// A UUIDv4 is always a valid segment, so this is infallible.");
        line("    pub fn mint() -> Self {");
        line(&format!(
            "        Self::from_parts({ri}::generate_system())"
        ));
        line("    }");
    }

    if let Some((parent_pattern_str, parent_name, parent_variables)) = &parent {
        line("");
        line(&format!(
            "    /// The parent resource name, `{parent_pattern_str}`."
        ));
        line(&format!("    pub fn parent(&self) -> {parent_name} {{"));
        line(
            "        // Each variable was validated at construction, so the parent name is valid.",
        );
        let args: Vec<String> = parent_variables
            .iter()
            .map(|var| format!("self.{var}.clone()"))
            .collect();
        // The `.expect(...)` always overflows one line, so the call breaks. When
        // the arguments fit on the `::new(...)` line, rustfmt indents `.expect`
        // one level under the receiver; when they don't, the broken `)` returns
        // to the method's indent and `.expect` follows at that indent.
        let new_call = format!("        {parent_name}::new({})", args.join(", "));
        if new_call.len() <= RUSTFMT_MAX_WIDTH {
            line(&new_call);
            line("            .expect(\"a validated resource name has a valid parent\")");
        } else {
            line(&format!("        {parent_name}::new("));
            for arg in &args {
                line(&format!("            {arg},"));
            }
            line("        )");
            line("        .expect(\"a validated resource name has a valid parent\")");
        }
        line("    }");

        // `mint_under` — infallible constructor for parented wrappers.
        let ri = &paths.resourceid;
        line("");
        line("    /// Mint a resource name under `parent` with a system-assigned ID (AIP-148).");
        line("    /// A UUIDv4 is always a valid segment, so this is infallible.");
        line(&format!(
            "    pub fn mint_under(parent: &{parent_name}) -> Self {{"
        ));
        let mut mint_args: Vec<String> = parent_variables
            .iter()
            .map(|var| format!("parent.{var}().to_owned()"))
            .collect();
        mint_args.push(format!("{ri}::generate_system()"));
        let mint_one_line = format!("        Self::from_parts({})", mint_args.join(", "));
        if mint_one_line.len() <= RUSTFMT_MAX_WIDTH {
            line(&mint_one_line);
        } else {
            line("        Self::from_parts(");
            for arg in &mint_args {
                line(&format!("            {arg},"));
            }
            line("        )");
        }
        line("    }");
    }
    line("}");

    // `Display` — writes the canonical name stored at construction, no per-call
    // re-format (the `from_parts` constructor already paid that cost once).
    line("");
    line(&format!("impl ::std::fmt::Display for {struct_name} {{"));
    line("    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {");
    line("        f.write_str(&self.name)");
    line("    }");
    line("}");

    // `AsRef<str>` — the canonical name as `&str`, so the wrapper drops into any
    // `impl AsRef<str>` API (map keys, comparisons) without an explicit `as_str`.
    line("");
    line(&format!("impl AsRef<str> for {struct_name} {{"));
    line("    fn as_ref(&self) -> &str {");
    line("        &self.name");
    line("    }");
    line("}");

    // `Ord` / `PartialOrd` — ordered by the canonical resource name's *string*
    // order, NOT a field-tuple derive. The two diverge when one variable value is
    // a prefix of another: `a` vs `a-b` has `'-' < '/'`, so `…/a-b/…` sorts before
    // `…/a/…` as strings, while a `(var, …)` tuple sorts `a` first. String order
    // matches `BTreeMap<String, _>` and SQL `ORDER BY name`, so a wrapper-keyed
    // map lists in the same order the demo's name-keyed map did. Consistent with
    // the derived `Eq` (the stored name determines the variables 1:1).
    line("");
    line(&format!("impl Ord for {struct_name} {{"));
    line("    /// Orders by the canonical resource name string — the order a");
    line("    /// `BTreeMap<String, _>` or SQL `ORDER BY name` produces, not the");
    line("    /// variable-tuple order (which diverges when one value is a prefix of");
    line("    /// another, e.g. `a` vs `a-b`).");
    line("    fn cmp(&self, other: &Self) -> ::std::cmp::Ordering {");
    line("        self.name.cmp(&other.name)");
    line("    }");
    line("}");
    line("");
    line(&format!("impl PartialOrd for {struct_name} {{"));
    line("    fn partial_cmp(&self, other: &Self) -> Option<::std::cmp::Ordering> {");
    line("        Some(self.cmp(other))");
    line("    }");
    line("}");

    line("");
    line(&format!("impl ::std::str::FromStr for {struct_name} {{"));
    line(&format!("    type Err = {rn}::Error;"));
    line("");
    line("    fn from_str(s: &str) -> Result<Self, Self::Err> {");
    line("        Self::parse(s)");
    line("    }");
    line("}");

    // `TryFrom<&str>` / `TryFrom<String>` — std pairs these with `FromStr` so
    // call sites can `.try_into()` and generic `T: TryFrom<&str>` bounds bind.
    // Both delegate to `parse`, carrying the same `Error` as `FromStr`.
    line("");
    line(&format!("impl TryFrom<&str> for {struct_name} {{"));
    line(&format!("    type Error = {rn}::Error;"));
    line("");
    line("    fn try_from(s: &str) -> Result<Self, Self::Error> {");
    line("        Self::parse(s)");
    line("    }");
    line("}");

    line("");
    line(&format!("impl TryFrom<String> for {struct_name} {{"));
    line(&format!("    type Error = {rn}::Error;"));
    line("");
    line("    fn try_from(s: String) -> Result<Self, Self::Error> {");
    line("        Self::parse(&s)");
    line("    }");
    line("}");

    // `From<Wrapper> for String` — hands back the stored canonical name by move,
    // no re-format or extra allocation.
    line("");
    line(&format!("impl From<{struct_name}> for String {{"));
    line(&format!("    fn from(name: {struct_name}) -> Self {{"));
    line("        name.name");
    line("    }");
    line("}");
    Ok(())
}

/// rustfmt's default `max_width`. Generated bracketed lists and signatures
/// collapse to one line at or under this width and break above it, matching
/// rustfmt so the emitted code is already formatted.
const RUSTFMT_MAX_WIDTH: usize = 100;

/// rustfmt's default `array_width` (under `use_small_heuristics = "Default"`): an
/// array literal wider than this breaks to one element per line even when the
/// whole line would still fit within [`RUSTFMT_MAX_WIDTH`].
const RUSTFMT_ARRAY_WIDTH: usize = 60;

/// `PascalCase` -> `SCREAMING_SNAKE_CASE` for the per-wrapper `LazyLock` const
/// name (`ShipperResourceName` -> `SHIPPER_RESOURCE_NAME`). The struct names are
/// ASCII `PascalCase`, so this only needs to split before each interior capital.
fn screaming_snake(pascal: &str) -> String {
    let mut out = String::new();
    for (i, c) in pascal.char_indices() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// The parent pattern of `pattern` — the pattern with its trailing
/// collection+variable pair dropped (`shippers/{shipper}/sites/{site}` ->
/// `shippers/{shipper}`) — or `None` if `pattern` has fewer than two such pairs
/// (a top-level resource has no parent to generate).
fn parent_pattern(pattern: &str) -> Option<String> {
    let segments: Vec<&str> = pattern.split('/').collect();
    if segments.len() < 4 {
        return None;
    }
    Some(segments[..segments.len() - 2].join("/"))
}

/// The variable names of `pattern`, in order, read out over the runtime's own
/// [`Scanner`]/`Segment` API. Parsing first validates the pattern (rejecting
/// wildcards, duplicate variables, …) so the emitted `PATTERN` const is one the
/// runtime can re-parse.
fn pattern_variables(pattern: &str) -> Result<Vec<String>, Error> {
    Pattern::parse(pattern)?;
    let mut scanner = Scanner::new(pattern);
    let mut variables = Vec::new();
    while let Some(segment) = scanner.scan() {
        if segment.is_variable() {
            variables.push(segment.literal().0.to_owned());
        }
    }
    Ok(variables)
}

#[cfg(test)]
mod tests {
    use super::{rust_path, snake_module};

    #[test]
    fn snake_module_splits_pascal_case() {
        assert_eq!(snake_module("MethodSettings"), "method_settings");
        assert_eq!(snake_module("FunctionDecl"), "function_decl");
        assert_eq!(snake_module("CreateStruct"), "create_struct");
        assert_eq!(snake_module("Expr"), "expr");
        // Acronyms: matches prost-build (heck) — xml_http_request not x_m_l_http_request.
        assert_eq!(snake_module("XMLHttpRequest"), "xml_http_request");
        assert_eq!(snake_module("HTTPConfig"), "http_config");
    }

    /// A message-name module that is a Rust keyword must match prost: `r#kw` for
    /// the raw-able keywords (the common `Type` case), a trailing `_` for the
    /// four that cannot be raw identifiers.
    #[test]
    fn snake_module_sanitizes_keywords() {
        assert_eq!(snake_module("Type"), "r#type");
        assert_eq!(snake_module("Match"), "r#match");
        // `gen` is a 2024 reserved keyword prost raw-escapes (prost-build 0.14).
        assert_eq!(snake_module("Gen"), "r#gen");
        assert_eq!(snake_module("Self"), "self_");
        assert_eq!(snake_module("Super"), "super_");
        assert_eq!(snake_module("Crate"), "crate_");
    }

    #[test]
    fn rust_path_snakes_parents_and_keeps_the_leaf() {
        assert_eq!(rust_path(&["Shipper".to_owned()]), "Shipper");
        assert_eq!(
            rust_path(&["Outer".to_owned(), "Inner".to_owned()]),
            "outer::Inner"
        );
        assert_eq!(
            rust_path(&[
                "Decl".to_owned(),
                "FunctionDecl".to_owned(),
                "Overload".to_owned()
            ]),
            "decl::function_decl::Overload"
        );
        // Keyword parent, verbatim leaf.
        assert_eq!(
            rust_path(&["Type".to_owned(), "AbstractType".to_owned()]),
            "r#type::AbstractType"
        );
    }
}
