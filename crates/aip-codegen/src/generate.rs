//! The generation logic: walk `google.api.resource` descriptors and emit typed
//! resource-name wrappers layered on the runtime [`aip_resourcename::Pattern`]
//! API (ADR-0011), and walk **Request descriptors** and emit
//! `impl aip_pagination::PageRequest` / `impl aip_ordering::OrderByRequest`
//! keyed on field shape (ADR-0013).
//!
//! Generated files are `use`-free and fully path-qualified
//! (`::aip_resourcename::…`, `::aip_pagination::…`), so the consumer mounts
//! every `*.aip.rs` directly in the module holding the prost message structs —
//! one mount rule whether the file carries wrappers, trait impls, or both
//! (ADR-0013). A trait impl names its prost struct (and a wrapper's [`parent`]
//! its parent wrapper) by bare path in that shared module.
//!
//! Each resource yields a `<Type>ResourceName` struct — one **private** `String`
//! field per pattern variable — built through a validated `new(...) ->
//! Result<Self, Error>` constructor (each variable checked with
//! [`aip_resourcename::validate_variable`]). Because construction validates every
//! variable, the wrapper formats infallibly: it implements [`Display`], plus
//! [`FromStr`] (delegating to `parse`) and `From<Self> for String`. The compiled
//! [`Pattern`] is built once per wrapper in a `LazyLock`, shared by `parse` and
//! `Display`. The generator does **not** reimplement the runtime; the emitted
//! code calls into it.
//!
//! A multi-segment wrapper also gets a [`parent`] accessor returning the parent
//! pattern's typed wrapper when that parent pattern is generated in the same
//! invocation.
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

use aip_reflect::{RequestDescriptor, ResourceDescriptor, ResourceType};
use aip_resourcename::{Pattern, Scanner};

use crate::Error;

/// Crate-root paths used in generated code, so the consumer can route
/// references through the `aip` umbrella (`aip_crate=aip` -> `::aip::pagination::…`)
/// instead of the per-crate names (`::aip_pagination::…`).
///
/// Construct with [`CratePaths::default`] for per-crate paths, or
/// [`CratePaths::from_aip_crate`] with an umbrella root like `"aip"`.
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
}

impl Default for CratePaths {
    fn default() -> Self {
        Self {
            resourcename: "::aip_resourcename".to_owned(),
            resourceid: "::aip_resourceid".to_owned(),
            pagination: "::aip_pagination".to_owned(),
            ordering: "::aip_ordering".to_owned(),
        }
    }
}

impl CratePaths {
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
}

/// Generate typed resource-name wrappers and request-trait impls, one output
/// [`GenFile`] per input file whose body carries at least one of either.
///
/// A resource with no patterns contributes nothing, as does a request without
/// the pagination field shape; a file contributing neither produces no file at
/// all (matching aip-go).
pub fn generate(inputs: &[GenInput], paths: &CratePaths) -> Result<Vec<GenFile>, Error> {
    // A `pattern -> <Type>ResourceName` index over every resource in the whole
    // invocation, so a multi-segment wrapper can resolve its parent pattern to
    // the parent's typed wrapper even when that parent lives in another file.
    let wrappers_by_pattern = index_wrappers(inputs)?;

    let mut files = Vec::new();
    for input in inputs {
        if let Some(content) = generate_file(
            &input.resources,
            &input.requests,
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

/// Generate the body of one file's resource-name wrappers and request-trait
/// impls, or `None` if it would be empty.
///
/// `wrappers_by_pattern` is the whole-invocation `pattern -> struct name` index
/// used to resolve a multi-segment wrapper's [`parent`](https://google.aip.dev/122).
fn generate_file(
    resources: &[ResourceDescriptor],
    requests: &[RequestDescriptor],
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
    }
    for request in requests {
        if request.has_page_token && request.has_page_size {
            write_page_request(&mut body, request, paths);
        }
        if request.has_order_by {
            write_order_by_request(&mut body, request, paths);
        }
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
    line(&format!("        Ok(Self {{ {} }})", variables.join(", ")));
    line("    }");

    for var in &variables {
        line("");
        line(&format!("    /// The `{{{var}}}` variable."));
        line(&format!("    pub fn {var}(&self) -> &str {{"));
        line(&format!("        &self.{var}"));
        line("    }");
    }

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
    line("        Ok(Self {");
    for var in &variables {
        line(&format!("            {var}: captures"));
        line(&format!("                .get(\"{var}\")"));
        line(&format!(
            "                .ok_or_else(|| {rn}::Error::MissingVariable {{"
        ));
        line(&format!("                    name: \"{var}\".to_owned(),"));
        line("                })?");
        line("                .to_owned(),");
    }
    line("        })");
    line("    }");

    // `parse_field` — validates the value as a resource name then matches the
    // pattern, wrapping every error with the request field path as a
    // `FieldError`. Handlers parse once and propagate via `?` to get the right
    // AIP-193 `BadRequest` field violation.
    line("");
    line("    /// Parse `value` from request field `field`, wrapping any error with");
    line("    /// the field path so `?` produces an AIP-193 `BadRequest` violation.");
    line(&format!(
        "    pub fn parse_field(field: &str, value: &str) -> Result<Self, {rn}::FieldError> {{"
    ));
    line("        let wrap = |source| {");
    line(&format!(
        "            {rn}::FieldError {{ field: field.to_owned(), source }}"
    ));
    line("        };");
    line(&format!("        {rn}::validate(value).map_err(wrap)?;"));
    line("        Self::parse(value).map_err(wrap)");
    line("    }");

    // `mint` — infallible constructor for single-variable (no parent) wrappers
    // only; `mint_under` is the parented variant.
    if parent.is_none() && variables.len() == 1 {
        let last_var = variables.last().expect("at least one variable");
        let ri = &paths.resourceid;
        line("");
        line("    /// Mint a resource name with a system-assigned ID (AIP-148).");
        line("    /// A UUIDv4 is always a valid segment, so this is infallible.");
        line("    pub fn mint() -> Self {");
        line("        Self {");
        line(&format!("            {last_var}: {ri}::generate_system(),"));
        line("        }");
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
        let last_var = variables.last().expect("at least one variable");
        let ri = &paths.resourceid;
        line("");
        line("    /// Mint a resource name under `parent` with a system-assigned ID (AIP-148).");
        line("    /// A UUIDv4 is always a valid segment, so this is infallible.");
        line(&format!(
            "    pub fn mint_under(parent: &{parent_name}) -> Self {{"
        ));
        line("        Self {");
        for var in parent_variables {
            line(&format!("            {var}: parent.{var}().to_owned(),"));
        }
        line(&format!("            {last_var}: {ri}::generate_system(),"));
        line("        }");
        line("    }");
    }
    line("}");

    // `Display` — infallible, since construction validated every variable. Emits
    // the `[(var, value), …]` array on one line when it fits, else one per line.
    line("");
    line(&format!("impl ::std::fmt::Display for {struct_name} {{"));
    line("    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {");
    line("        // Construction validated each variable, so formatting is infallible.");
    let pairs: Vec<String> = variables
        .iter()
        .map(|var| format!("(\"{var}\", self.{var}.as_str())"))
        .collect();
    let one_line = format!(
        "        f.write_str(&{pattern_const}.format([{}]).expect(\"a validated resource name formats\"))",
        pairs.join(", ")
    );
    if one_line.len() <= RUSTFMT_MAX_WIDTH {
        line(&one_line);
    } else {
        // The chain breaks across lines; the `.format([…])` array stays inline
        // while its literal fits rustfmt's `array_width`, else one pair per line.
        line("        f.write_str(");
        line(&format!("            &{pattern_const}"));
        let array = format!("[{}]", pairs.join(", "));
        if array.len() <= RUSTFMT_ARRAY_WIDTH {
            line(&format!("                .format({array})"));
        } else {
            line("                .format([");
            for pair in &pairs {
                line(&format!("                    {pair},"));
            }
            line("                ])");
        }
        line("                .expect(\"a validated resource name formats\"),");
        line("        )");
    }
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

    line("");
    line(&format!("impl From<{struct_name}> for String {{"));
    line(&format!("    fn from(name: {struct_name}) -> Self {{"));
    line("        name.to_string()");
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
