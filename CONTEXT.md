# aip-rs

A Rust SDK for Google's API Improvement Proposals (AIP). This glossary fixes the
domain language shared across the crates so the same word never means two things.

## Language

### Resource names

**Resource name**:
A `/`-separated identifier for a single resource, e.g. `shippers/123/shipments/456`.
_Avoid_: path, URI, key.

**Pattern**:
A resource-name template whose identifier positions are **Variables**, e.g.
`shippers/{shipper}/shipments/{shipment}`. A **Pattern** is matched against a
**Resource name**; a **Resource name** is concrete.
_Avoid_: template, format string, schema.

**Segment**:
One `/`-separated component of a **Resource name** or **Pattern**. Every segment
is a **Literal**, a **Variable**, or a **Wildcard**.

**Literal**:
A segment with a fixed value — either a **Collection ID** or a **Resource ID**,
e.g. `shippers` or `123`.
_Avoid_: constant, token.

**Variable**:
A `{name}` placeholder segment that appears only in a **Pattern** and binds the
value matched in that position.
_Avoid_: parameter, placeholder, capture group.

**Wildcard**:
The `-` segment that stands for "any resource ID" inside a **Resource name**,
used to refer to all resources in a collection (e.g. `shippers/-/shipments/-`).
Distinct from a **Variable**: a Wildcard lives in a name and matches anything; a
Variable lives in a Pattern and binds a value.
_Avoid_: star, glob, any.

**Collection ID**:
A **Literal** naming a collection — the plural nouns that alternate with
**Resource IDs**, e.g. `shippers`, `shipments`.

**Resource ID**:
The identifier of a resource within its collection — a **Literal** with any
**Revision ID** stripped.
_Avoid_: uid, primary key.

**Revision ID**:
The portion of a **Literal** after the `@` separator, e.g. `1.0.0` in
`books/les-miserables@1.0.0`.
_Avoid_: version, tag.

**Full resource name**:
A **Resource name** prefixed with a **Service name**, e.g.
`//library.googleapis.com/shelves/1`.
_Avoid_: absolute name, URL.

**Service name**:
The DNS-style host that prefixes a **Full resource name**, e.g.
`library.googleapis.com`.

### Pagination

**Page token**:
An opaque string returned by a list method that, when passed back, fetches the
next page of results. Carries enough state to resume and to detect that the rest
of the request changed.
_Avoid_: cursor (that's one *kind* of page token), continuation, offset.

**Offset page token**:
A **Page token** whose state is a numeric offset into the result set.

**Cursor page token**:
A **Page token** whose state is an arbitrary, caller-defined payload (e.g. the
last-seen key) rather than an offset.

**Page size**:
The maximum number of results a single page may contain, requested by the client.

**Size limits**:
A list method's AIP-158 page-size policy: the **Page size** the server picks when
the client leaves one unset (the *default*), and the ceiling no single page may
exceed (the *max*). A negative requested **Page size** is rejected; zero takes the
default; the result is capped at the max.
_Avoid_: policy (that's the IAM term), page config, bounds.

**Page**:
The resolved AIP-158 pagination state for one list page: the verified **Offset
page token** paired with the effective **Page size** after the **Size limits**
default/cap is applied. Folds the request-consistency check, **Page token**
verification, and size resolution into one value the list handler reads. Exposes
that state through unsigned `offset()` / `size()` accessors (the signed token
fields stay internal) and **owns applying itself to the results** — slicing an
in-memory collection or splitting a store-backed **Overfetch probe** — minting the
next **Page token** in the same step.
_Avoid_: page state, cursor, result set.

**Skip**:
A count of leading results to discard before the page begins (AIP-158 skip),
applied on top of a **Page token**'s position.

**Overfetch probe**:
Fetching one row past the **Page size** so the extra row's *presence* tells a list
handler another page remains. The probe row is truncated off before the response,
and its presence mints the next **Page token**. The store-backed partner to
applying a **Page** to a collection already in memory.
_Avoid_: peek, lookahead, sentinel row.

### Field masks

**Field mask**:
A list of field paths (e.g. `display_name`, `book.author`) naming a subset of a
message's fields. The umbrella term; its meaning depends on whether it is used to
write (**Update mask**) or read (**Read mask**).
_Avoid_: projection, selection.

**Update mask**:
A **Field mask** on a write that names exactly which fields to change. A path
absent from the mask is left untouched; a masked path absent from the source is
cleared.

**Read mask**:
A **Field mask** on a read that names which fields to return. (Recognised as a
distinct concept; response projection is deferred beyond v0.1.)

**Full replacement**:
The special **Update mask** `*`, meaning "replace every field" rather than a
selective update.

### Ordering

**Order by**:
An AIP-132 directive listing the fields, each ascending or descending, by which a
list method's results are sorted. Parsed and validated here; the sort itself is
performed by the datastore.
_Avoid_: sort, sort order.

**Ordering field**:
One field path plus its direction (ascending/descending) within an **Order by**.
A path may address a **Subfield** with `.` (e.g. `author.name`).

### Filtering

**Filter**:
An AIP-160 expression over a resource's fields. Parsed and type-checked into an
AST here; evaluation/translation is left to the caller.
_Avoid_: query, predicate, where clause.

**Declaration**:
The typed schema — the filterable **Identifiers**, functions, and enums — that a
**Filter** is checked against. Acts as an allowlist of what may be filtered.
_Avoid_: schema, environment.

**Identifier**:
A name **Declared** as filterable, paired with a **Type**.

**Has operator**:
The `:` operator in a **Filter**, testing presence/membership (e.g. a key in a
map, a value in a list).

### IAM

The optional `aip-iam` crate (ADR-0010): parses and validates the `google.iam.v1`
identity vocabulary. Like the rest of the core it *parses and validates* — the
authorization **decision** (role→permission expansion, condition evaluation) is
left to the caller.

**Member**:
A principal a **Binding** grants a **Role** to, written in the `type:value` grammar
(`user:`, `serviceAccount:`, `group:`, `domain:`, or the bare `allUsers` /
`allAuthenticatedUsers`).
_Avoid_: principal (the generic term), identity, subject, account.

**Role**:
A named bundle of **Permissions**, in one of three forms: predefined
(`roles/{role}`) or custom, scoped to a project (`projects/{p}/roles/{r}`) or an
organization (`organizations/{o}/roles/{r}`).
_Avoid_: grant, group (a **Member** kind), capability.

**Permission**:
A single `service.resource.verb` unit of access (e.g. `freight.shippers.get`) that
a **Role** bundles.
_Avoid_: scope, action, right, privilege.

**Policy**:
The `google.iam.v1.Policy` attached to a **Resource name** — a set of **Bindings**
plus an `etag` for read-modify-write. (Structure deferred past the parsing slice;
ADR-0010.)
_Avoid_: ACL, ruleset, permissions.

**Binding**:
One entry in a **Policy** pairing a **Role** with the **Members** it is granted to,
optionally gated by a **Condition**.
_Avoid_: grant, rule, ACE.

**Condition**:
A `google.type.Expr` (CEL) predicate gating a **Binding**. Bridged to the
**Filter** AST via `aip-filtering`'s CEL interop; evaluation is the caller's
(ADR-0010).
_Avoid_: filter (a distinct AIP-160 concept), guard, rule.

### Events

The planned `aip-events` crate (issue #103): resource-centric change events. No
published AIP standardizes an event bus; the precedent followed is CloudEvents
with protobuf payloads (Eventarc style).

**Change event**:
A record that one resource was created, updated, deleted, or undeleted —
identified by its **Resource name** and carrying the post-change resource.
Carried in a CloudEvents envelope.
_Avoid_: notification, message, event (unqualified, where ambiguous).

**Change kind**:
Which of the four changes a **Change event** records: created, updated, deleted,
or undeleted. Encoded as the verb suffix of the event's type string, which is
derived per resource from its AIP-123 resource type.
_Avoid_: event type (that's the full derived string), action, operation, verb.

**Subscription**:
A standing **Filter** over **Change events**, checked against a **Declaration**
of the envelope fields plus — when the Subscription names a resource type — that
resource's fields under a `resource.` prefix. A Change event is delivered to a
Subscription when its Filter matches.
_Avoid_: watch (the RPC verb, not the standing filter), listener, query.

### SQL adapter

The optional `aip-sql` crate (ADR-0005 / ADR-0008): turns a primitive's native
AST into parameterized SQL. Not part of the database-agnostic core.

**Predicate**:
A composable, parameterized boolean SQL fragment — the **Transpiled** form of a
**Filter**. Its logical structure (`AND`/`OR`/`NOT`) is portable; its leaves are
spelled by a **Dialect**. Owns precedence and placeholder numbering so a server
can compose a user's **Filter** with its own predicates safely. A Predicate is
the WHERE *within* a **Query**, not the whole query.
_Avoid_: where clause, condition, expression, and **Query** (the Predicate is one
part of a Query, not a synonym for it).

**Dialect**:
The per-engine renderer that spells a **Predicate**'s leaves and numbers its
placeholders (`?n` for SQLite, `$n` for Postgres), rendering a **Predicate** to
`(sql, bind values)` in a single pass. SQLite first, then Postgres.
_Avoid_: backend, driver, adapter.

**Bind value**:
An executor-agnostic literal lifted out of a **Filter** and bound as a SQL
parameter — never spliced into SQL text (parameterize, never interpolate). The
caller binds it to whatever driver it uses.
_Avoid_: parameter (ambiguous), argument, literal.

**Transpile**:
To walk a primitive's native AST — a **Filter**, an **Order by** — into a
**Predicate**. The `aip-sql` operation; distinct from **Filter** parsing and
checking, which stay reflection-free and datastore-free.
_Avoid_: compile, translate, convert.

**Query**:
The clause tail of a list query, rendered by a **Dialect** in one pass: the WHERE
**Predicate**, the **Order by** columns, and the `LIMIT` / `OFFSET` page tail
bundled and rendered to one `(sql, bind values)`. Only the WHERE contributes
**Bind values**. A Query owns no `SELECT` / `FROM` — the table and projection are
the caller's — so the caller writes the head and interpolates the Query's tail.
_Avoid_: statement (a Query carries no `SELECT` / `FROM`), where clause (that is
the **Predicate**).

### Reflection

**Descriptor**:
The reified protobuf message type — a `prost_reflect::MessageDescriptor`. Both a
**Typed message** (which carries its own) and a **Dynamic message** are described
by one; resolving a **Field mask** or **Order by** path means walking a descriptor.
_Avoid_: schema, type info, reflection data.

**Reflective primitive**:
An AIP operation that needs a message's **Descriptor** to run — applying an
**Update mask**, computing a **Page token**'s request checksum. The pure-syntax
operations (**Resource name**, **Resource ID**) never reflect.
_Avoid_: dynamic operation, introspective primitive.

**Typed message**:
A concrete generated prost message that carries its own **Descriptor** because it
implements `prost_reflect::ReflectMessage`. Every **Reflective primitive**'s
**Typed facade** is expressed over typed messages, so a caller never builds or
threads a descriptor pool, nor touches a **Dynamic message**.
_Avoid_: concrete message (only when contrasting), native message.

**Dynamic message**:
A `prost_reflect::DynamicMessage` — a message value whose type is known only at
runtime through a **Descriptor**. The surface beneath each **Typed facade**, and
the surface the reflective crates test through (JSON → a dynamic message, via
`test-fixtures`).
_Avoid_: reflected message, generic message, untyped message.

**Typed facade / Dynamic core**:
The two surfaces of a **Reflective primitive**. The *typed facade* is the
headline interface over **Typed messages**; the *dynamic core* is the public
low-level interface over **Dynamic messages** that the facade transcodes onto. The
core is also the test surface and the escape hatch for a caller who holds only a
**Dynamic message** (JSON ingestion, a generic gateway).
_Avoid_: wrapper/inner, high-level/low-level (say facade/core).

**Request descriptor**:
The profile of which AIP-standard pagination (**Page size**, **Page token**,
**Skip**) and **Ordering** (**Order by**) fields a request message carries, read
by **field shape** — each field counts only when its name *and* type match — not
by the method the request serves. It is what drives whether codegen emits the
pagination/ordering request traits for that message. Distinct from a
**Descriptor**: a Request descriptor is an aip-rs digest of standard-field
presence over one message, not a `prost_reflect::MessageDescriptor`.
_Avoid_: descriptor (the reflected message type), method type (a Request
descriptor is read from field shape, not from the method's identity).

## Relationships

- An **Order by** is an ordered list of **Ordering fields**.
- A **Filter** is checked against a **Declaration**; every **Identifier** it
  references must be declared.
- A **Filter** is **Transpiled** into a **Predicate**; a **Dialect** renders a
  **Predicate** to SQL text plus an ordered list of **Bind values**.
- A **Query** bundles a **Predicate** (the WHERE), the **Order by** columns, and a
  `LIMIT` / `OFFSET` page tail; a **Dialect** renders it to one clause tail plus
  an ordered list of **Bind values** (only the WHERE binds). The `SELECT` /
  `FROM` head is the caller's.
- A **Field mask** is interpreted as either an **Update mask** or a **Read mask**.
- A **Policy** is a set of **Bindings**; each **Binding** pairs one **Role** with
  the **Members** granted it, optionally gated by a **Condition**.
- A **Role** bundles **Permissions**; a **Permission** is a `service.resource.verb`.
- A **Change event** records exactly one **Change kind** for exactly one
  **Resource name** and carries the post-change resource and, for updates, the
  **Update mask** of changed paths.
- A **Subscription** is a **Filter** over **Change events**, checked against a
  **Declaration**; matching decides delivery, transport delivers.
- A **Page token** is either an **Offset page token** or a **Cursor page token**.
- A **Request descriptor** records, by **field shape**, which **Page size** /
  **Page token** / **Skip** / **Order by** fields a request message carries;
  codegen emits the matching pagination/ordering request traits from it.
- A **Resource name** is an ordered sequence of **Segments** joined by `/`.
- A **Segment** is exactly one of: **Literal**, **Variable**, **Wildcard**.
- A **Pattern** is matched against a **Resource name** to bind each **Variable**
  to the **Resource ID** in that position.
- A **Literal** carries a **Resource ID** and may carry a **Revision ID** after `@`.
- **Collection IDs** and **Resource IDs** alternate within a well-formed
  **Resource name**.
- A **Full resource name** is a **Service name** plus a **Resource name**.
- A **Reflective primitive** presents a **Typed facade** layered on a **Dynamic
  core**; the facade transcodes a **Typed message** to a **Dynamic message** and
  back through wire bytes.
- A **Typed message** carries its own **Descriptor**; a **Dynamic message** is
  paired with one.

## Example dialogue

> **Dev:** "Does `Match` take two **Resource names**?"
> **Domain expert:** "No — the first argument is a **Pattern** (it has
> **Variables** like `{shipper}`), the second is a concrete **Resource name**.
> Matching binds each **Variable** to the **Resource ID** in that position."
> **Dev:** "And `shippers/-/shipments/-`?"
> **Domain expert:** "That's a **Resource name** with **Wildcards**, not a
> **Pattern**. `-` means 'any resource ID'; it binds nothing."

## Flagged ambiguities

- "name" was used for both a concrete **Resource name** and a **Pattern** —
  resolved: a Pattern has **Variables**; a Resource name is concrete.
- "wildcard" vs "variable" — resolved: a **Wildcard** (`-`) appears in a
  **Resource name** and matches any value; a **Variable** (`{x}`) appears in a
  **Pattern** and binds a value.
