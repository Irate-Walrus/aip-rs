//! Datastore backing the demo.
//!
//! Shippers live in an in-memory map (ADR-0005); the gRPC layer — not the store —
//! is what exercises those primitives. Sites and shipments are backed by an
//! **in-memory SQLite database** so an AIP-160 **Filter** travels end-to-end into a
//! real SQL engine (ADR-0008): this is the default store, no feature flag. IAM
//! **Policies** are likewise SQLite-backed, decomposed into the `iam_policy_bindings`
//! table the way iam-go's `iamspanner` stores them (see [`PolicyStore`], aip #65).
//! State lives for the life of the process and resets on restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use prost::Message as _;

use crate::proto::einride::example::freight::v1::{
    site::State, Shipment, Shipper, ShipperResourceName, Site,
};
// `Policy` / `Binding` are aip-proto's generated `google.iam.v1` types (ADR-0011)
// — the very ones the `aip::iam` structural helpers operate on and the `IAMPolicy`
// service trait speaks, so there is one `Policy` type by construction (aip #65,
// #82). `Expr` is the `google.type.Expr` **Condition** a `Binding` carries —
// persisted (its expression) and reconstructed so `TestIamPermissions` can
// evaluate it (aip #68).
use aip::iam::proto::{google::r#type::Expr, Binding, Policy};

/// Process-lifetime store. Shippers are a `BTreeMap` keyed by the typed
/// `ShipperResourceName` (issue #169) — its `Ord` is the canonical-name string
/// order, which keeps listings in a stable order (the deterministic tie-break
/// behind a stable `order_by` sort). Sites and shipments live in SQLite tables — the
/// filterable/sortable columns plus the full resource as wire bytes — so an
/// AIP-160 **Filter** composed with the server's own predicates (parent scope,
/// soft delete) travels end-to-end into a real SQL engine (ADR-0008).
pub struct Storage {
    shippers: Mutex<BTreeMap<ShipperResourceName, Shipper>>,
    sites: Mutex<rusqlite::Connection>,
    shipments: Mutex<rusqlite::Connection>,
    /// AIP-155 idempotency cache (issue #94): per `request_id`, the wire bytes of
    /// the create request that first used it and of the response it produced.
    /// The library validates the id and names the [`Replay`] contract; this cache
    /// of seen ids is the server's, matching the parse-and-validate boundary.
    /// In-memory and process-lifetime like the rest of the store.
    ///
    /// The lookup and the record are two separate lock acquisitions (the create
    /// work between them touches no shared state), so two *concurrent* creates
    /// with the same brand-new `request_id` can both miss and both proceed — the
    /// demo does not reserve an id across the whole handler. A production cache
    /// would close that window (a unique key on the seen-id row, or a reservation
    /// like [`PolicyStore::set_checked`]'s single-lock read-modify-write);
    /// AIP-155 leaves the timeframe and concurrency policy to the service.
    ///
    /// [`Replay`]: aip::requestid::Replay
    idempotency: Mutex<BTreeMap<String, IdempotentRecord>>,
}

/// One AIP-155 idempotency-cache entry (issue #94): the wire bytes of the create
/// request that first used a `request_id` and of the response it produced. The
/// `request` bytes let the server tell an identical replay from a conflicting
/// reuse; the `response` bytes are decoded back into the resource on a replay.
#[derive(Clone)]
pub struct IdempotentRecord {
    /// Wire bytes of the create request that first used the `request_id`.
    pub request: Vec<u8>,
    /// Wire bytes of the response that request produced.
    pub response: Vec<u8>,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage {
    /// An empty store, with fresh in-memory SQLite databases for sites and
    /// shipments.
    pub fn new() -> Self {
        Self {
            shippers: Mutex::new(BTreeMap::new()),
            sites: Mutex::new(new_sites_db()),
            shipments: Mutex::new(new_shipments_db()),
            idempotency: Mutex::new(BTreeMap::new()),
        }
    }

    /// The [`IdempotentRecord`] recorded the first time `request_id` was used on a
    /// create, or `None` if it is unseen (AIP-155, issue #94). The caller compares
    /// the stored request against the incoming one to tell an identical replay
    /// from a conflicting reuse.
    pub fn idempotent_get(&self, request_id: &str) -> Option<IdempotentRecord> {
        self.idempotency.lock().unwrap().get(request_id).cloned()
    }

    /// Record a create's request + response wire bytes under `request_id`, so a
    /// later retry with the same id replays the response instead of acting again
    /// (AIP-155, issue #94).
    pub fn idempotent_put(&self, request_id: String, request: Vec<u8>, response: Vec<u8>) {
        self.idempotency
            .lock()
            .unwrap()
            .insert(request_id, IdempotentRecord { request, response });
    }

    /// Fetch a shipper by its typed resource name (issue #169). The map is keyed
    /// by the wrapper, so the handler passes the `ShipperResourceName` it already
    /// parsed instead of a raw string.
    pub fn get_shipper(&self, name: &ShipperResourceName) -> Option<Shipper> {
        self.shippers.lock().unwrap().get(name).cloned()
    }

    /// Every shipper, in resource-name order. The typed key's `Ord` is the
    /// canonical-name string order (issue #169), so this is the same stable
    /// listing order the old `String`-keyed map produced.
    pub fn list_shippers(&self) -> Vec<Shipper> {
        self.shippers.lock().unwrap().values().cloned().collect()
    }

    /// Insert or overwrite a shipper under its typed `name`. Soft delete (AIP-164)
    /// stamps a `delete_time` and re-puts the shipper rather than removing it, so
    /// it stays addressable for `GetShipper`/`ListShippers` with `show_deleted`
    /// and recoverable by `UndeleteShipper` (#96) — there is no shipper removal.
    pub fn put_shipper(&self, name: &ShipperResourceName, shipper: Shipper) {
        self.shippers.lock().unwrap().insert(name.clone(), shipper);
    }

    /// Insert or overwrite a site, keyed by its `name`. The full site is stored as
    /// wire bytes alongside the columns an AIP-160 filter can address or an
    /// AIP-132 `order_by` can sort by: the scalar `display_name`, the timestamps
    /// `create_time` / `update_time` as sortable RFC3339 text, the nested
    /// `lat_lng.latitude` / `lat_lng.longitude` flattened to numeric columns, the
    /// enum `state` as its value name (matching the transpiler's enum rendering,
    /// #40), the `annotations` map / `tags` list as JSON the has operator queries
    /// with `json_each` (#41), and `delete_time` as RFC3339 text (NULL when live)
    /// so the server's soft-delete predicate `delete_time IS NULL` runs in SQL
    /// (#43).
    pub fn put_site(&self, site: Site) {
        let create_time = site.create_time.as_ref().map(aip::sql::format_timestamp);
        let update_time = site.update_time.as_ref().map(aip::sql::format_timestamp);
        let delete_time = site.delete_time.as_ref().map(aip::sql::format_timestamp);
        let latitude = site.lat_lng.as_ref().map(|ll| ll.latitude);
        let longitude = site.lat_lng.as_ref().map(|ll| ll.longitude);
        let state = State::try_from(site.state)
            .unwrap_or(State::Unspecified)
            .as_str_name();
        // The has operator reads these through SQLite's `json_each`, so they are
        // stored as JSON text — an object for the map, an array for the list.
        let annotations = serde_json::to_string(&site.annotations).expect("serialize annotations");
        let tags = serde_json::to_string(&site.tags).expect("serialize tags");
        self.sites
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO sites \
                 (name, display_name, create_time, update_time, delete_time, latitude, \
                  longitude, state, annotations, tags, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    site.name,
                    site.display_name,
                    create_time,
                    update_time,
                    delete_time,
                    latitude,
                    longitude,
                    state,
                    annotations,
                    tags,
                    site.encode_to_vec(),
                ],
            )
            .expect("insert site");
    }

    /// One page of sites matching `predicate`, sorted and paginated entirely in
    /// SQLite (#42). `predicate` is the server's composed `WHERE` — a parent
    /// scope, the soft-delete `delete_time IS NULL`, and any user filter (#43).
    ///
    /// See [`query_page`] for the query shape and the parameterize-never-interpolate
    /// guarantee.
    pub fn list_sites_page(
        &self,
        predicate: &aip::sql::Predicate,
        order_by: &[aip::sql::Order],
        limit: u64,
        offset: u64,
    ) -> Vec<Site> {
        let conn = self.sites.lock().unwrap();
        query_page(&conn, "sites", predicate, order_by, limit, offset)
            .into_iter()
            .map(|data| Site::decode(data.as_slice()).expect("decode site"))
            .collect()
    }

    /// Insert or overwrite a shipment, keyed by its `name`. The full shipment is
    /// stored as wire bytes alongside the columns an AIP-160 filter can address:
    /// the resource-name `name` (also the parent-scope and sort column), the
    /// `origin_site` / `destination_site` references, `create_time` as sortable
    /// RFC3339 text, the `annotations` map as JSON the has operator queries with
    /// `json_each`, and `delete_time` (NULL when live) for the soft-delete
    /// predicate (#43).
    pub fn put_shipment(&self, shipment: Shipment) {
        let create_time = shipment
            .create_time
            .as_ref()
            .map(aip::sql::format_timestamp);
        let delete_time = shipment
            .delete_time
            .as_ref()
            .map(aip::sql::format_timestamp);
        let annotations =
            serde_json::to_string(&shipment.annotations).expect("serialize annotations");
        self.shipments
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO shipments \
                 (name, origin_site, destination_site, create_time, delete_time, \
                  annotations, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    shipment.name,
                    shipment.origin_site,
                    shipment.destination_site,
                    create_time,
                    delete_time,
                    annotations,
                    shipment.encode_to_vec(),
                ],
            )
            .expect("insert shipment");
    }

    /// One page of shipments matching `predicate`, sorted and paginated entirely
    /// in SQLite — the same composed-`WHERE` path as
    /// [`list_sites_page`](Storage::list_sites_page) (#43).
    pub fn list_shipments_page(
        &self,
        predicate: &aip::sql::Predicate,
        order_by: &[aip::sql::Order],
        limit: u64,
        offset: u64,
    ) -> Vec<Shipment> {
        let conn = self.shipments.lock().unwrap();
        query_page(&conn, "shipments", predicate, order_by, limit, offset)
            .into_iter()
            .map(|data| Shipment::decode(data.as_slice()).expect("decode shipment"))
            .collect()
    }
}

/// Run one paginated list query against `table`, returning each matching row's
/// `data` blob in order. Shared by the site and shipment listings (#43).
///
/// The store owns only the `SELECT data FROM <table>` head; the WHERE / `ORDER
/// BY` / `LIMIT` / `OFFSET` tail is one [`aip::sql::Query`] rendered in a single
/// call. The composed `predicate` renders to parameterized SQL through the SQLite
/// [`Dialect`](aip::sql::Dialect) and its bind values are bound positionally —
/// never spliced into the SQL text (ADR-0005 / ADR-0008); the `ORDER BY` columns
/// (from the [`Schema`](aip::sql::Schema) allowlist) and the server-resolved
/// `LIMIT` / `OFFSET` integers carry no binds. `table` is a fixed string literal
/// supplied by the store, never request input.
fn query_page(
    conn: &rusqlite::Connection,
    table: &str,
    predicate: &aip::sql::Predicate,
    order_by: &[aip::sql::Order],
    limit: u64,
    offset: u64,
) -> Vec<Vec<u8>> {
    let (tail, binds) = aip::sql::Query::new()
        .filter(predicate.clone())
        .order_by(order_by.iter().cloned())
        .limit(limit)
        .offset(offset)
        .render(&aip::sql::Sqlite);
    let params: Vec<rusqlite::types::Value> = binds.into_iter().map(to_sql).collect();
    let sql = format!("SELECT data FROM {table} {tail}");
    let mut statement = conn.prepare(&sql).expect("prepare list query");
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .expect("run list query");
    rows.map(|data| data.expect("read list row")).collect()
}

/// Open an in-memory SQLite database with the `sites` table: the resource name as
/// primary key, the `display_name` / `create_time` / `update_time` / `latitude` /
/// `longitude` / `state` columns the AIP-160 filter addresses (#40) or the
/// AIP-132 `order_by` sorts by (#42), the JSON `annotations` / `tags` columns the
/// has operator queries with `json_each` (#41), the `delete_time` column behind
/// the server's soft-delete predicate (#43), and the full site as wire bytes for
/// lossless round-trips.
fn new_sites_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE sites (
            name         TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            create_time  TEXT,
            update_time  TEXT,
            delete_time  TEXT,
            latitude     REAL,
            longitude    REAL,
            state        TEXT NOT NULL,
            annotations  TEXT NOT NULL,
            tags         TEXT NOT NULL,
            data         BLOB NOT NULL
        );",
    )
    .expect("create sites table");
    conn
}

/// Open an in-memory SQLite database with the `shipments` table: the resource name
/// as primary key (also the parent-scope and sort column), the `origin_site` /
/// `destination_site` references and `create_time` the AIP-160 filter addresses,
/// the JSON `annotations` column the has operator queries with `json_each`, the
/// `delete_time` column behind the soft-delete predicate (#43), and the full
/// shipment as wire bytes for lossless round-trips.
fn new_shipments_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE shipments (
            name             TEXT PRIMARY KEY,
            origin_site      TEXT NOT NULL,
            destination_site TEXT NOT NULL,
            create_time      TEXT,
            delete_time      TEXT,
            annotations      TEXT NOT NULL,
            data             BLOB NOT NULL
        );",
    )
    .expect("create shipments table");
    conn
}

/// Process-lifetime store of `google.iam.v1.Policy`, backing the demo's
/// `IAMPolicy` service (aip #64, #65). Like a real IAM backend (and mirroring
/// iam-go's `iamspanner`), a **Policy** is stored *decomposed* into the
/// `iam_policy_bindings` table — one row per (resource, **Binding**, **Member**) —
/// rather than as an opaque blob; the **Policy** is reconstructed on read and its
/// `etag` is a content digest computed from that canonical form (ADR-0010).
///
/// Each row also carries its **Binding**'s **Condition** expression (the
/// `condition` column, NULL for an unconditional binding) so that the stored
/// **Policy** round-trips its **Conditions** and `TestIamPermissions` can evaluate
/// them through the opt-in `eval` adapter (aip #68). The policy `version` is *not*
/// stored — it is reconstructed from the *conditions ⟹ version 3* invariant
/// ([`reconstruct`]): version 3 when any binding is conditional, else version 1's
/// default. The invariant itself is still enforced by [`aip::iam::policy::validate`]
/// in the handler *before* a write, so a conditional binding on an old version is
/// rejected up front. A **Condition**'s `title` / `description` are not persisted
/// (only the `expression` the adapter needs). State lives for the process and
/// resets on restart, like the rest of [`Storage`].
pub struct PolicyStore {
    /// One in-memory SQLite connection holding the `iam_policy_bindings` table.
    bindings: Mutex<rusqlite::Connection>,
}

impl Default for PolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyStore {
    /// An empty policy store with a fresh in-memory `iam_policy_bindings` table.
    pub fn new() -> Self {
        Self {
            bindings: Mutex::new(new_policies_db()),
        }
    }

    /// The policy attached to `resource`, or `None` when none is set — the caller
    /// turns that into the empty `Policy` `GetIamPolicy` returns (AIP / IAM: an
    /// unset policy is not an error). The reconstructed policy carries the content
    /// `etag` ([`aip::iam::policy::compute`]), so a client can
    /// round-trip it back as its read-modify-write token.
    pub fn get(&self, resource: &str) -> Option<Policy> {
        let conn = self.bindings.lock().unwrap();
        let policy = reconstruct(&conn, resource);
        (!policy.bindings.is_empty()).then_some(policy)
    }

    /// Apply `SetIamPolicy` read-modify-write semantics atomically, the way a real
    /// IAM backend does it inside a transaction (aip #65).
    ///
    /// The supplied `policy.etag` is the client's optimistic-concurrency token: it
    /// is checked against the canonical stored policy via
    /// [`aip::iam::policy::check`] and a stale token is rejected with
    /// [`PolicyEtagMismatch`](aip::iam::Error::PolicyEtagMismatch) (`ABORTED`)
    /// rather than blind-overwriting. The accepted policy is folded into canonical
    /// form ([`normalize`](aip::iam::policy::normalize)) and its `(resource,
    /// binding, member)` rows replace the resource's previous rows in one
    /// transaction. The stored policy — `(role, members)` grants, with a fresh
    /// content `etag` — is returned so the caller can echo the new `etag`.
    ///
    /// The freshness check and the write share one lock acquisition, so two
    /// concurrent writers cannot both pass the check (no read-modify-write race).
    pub fn set_checked(
        &self,
        resource: String,
        mut policy: Policy,
    ) -> Result<Policy, aip::iam::Error> {
        // The client's etag is a concurrency token, not policy content.
        let supplied = std::mem::take(&mut policy.etag);
        let mut conn = self.bindings.lock().unwrap();

        // Freshness check against the canonical stored form — the same `reconstruct`
        // `get` uses, so the etag matches what a prior read handed the client.
        let current = reconstruct(&conn, &resource);
        let current = (!current.bindings.is_empty()).then_some(current);
        aip::iam::policy::check(&supplied, current.as_ref())?;

        // Canonicalise, then replace the resource's rows atomically. Each row
        // carries the binding's **Condition** expression (NULL when unconditional)
        // — the same value across every member of a binding, taken from whichever
        // row first reconstructs it on read (#68).
        aip::iam::policy::normalize(&mut policy);
        let tx = conn.transaction().expect("begin policy transaction");
        tx.execute(
            "DELETE FROM iam_policy_bindings WHERE resource = ?1",
            [&resource],
        )
        .expect("clear existing policy rows");
        for (binding_index, binding) in policy.bindings.iter().enumerate() {
            let condition = binding
                .condition
                .as_ref()
                .map(|expr| expr.expression.as_str());
            for (member_index, member) in binding.members.iter().enumerate() {
                tx.execute(
                    "INSERT INTO iam_policy_bindings \
                     (resource, binding_index, role, member_index, member, condition) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        resource,
                        binding_index as i64,
                        binding.role,
                        member_index as i64,
                        member,
                        condition
                    ],
                )
                .expect("insert policy binding row");
            }
        }
        tx.commit().expect("commit policy transaction");

        // Echo the canonical stored form exactly as `get` reconstructs it — the
        // `(role, members, condition)` grants with a fresh content `etag` — so the
        // etag the caller round-trips matches a subsequent `GetIamPolicy`.
        Ok(reconstruct(&conn, &resource))
    }
}

/// Reconstruct the canonical stored **Policy** for `resource` from its
/// `iam_policy_bindings` rows: the `(role, members, condition)` grants in canonical
/// order, with a fresh content `etag`. Returns the empty [`Policy`] when the
/// resource has no rows (the caller turns that into `None` / an empty
/// `GetIamPolicy` response).
///
/// `version` is not a stored column; it is recovered from the *conditions ⟹
/// version 3* invariant — version 3 when any reconstructed **Binding** is
/// conditional, else the default — so the round-tripped **Policy** is valid and its
/// `etag` (a digest over the whole **Policy**, including `version`) is stable across
/// `get` and the [`PolicyStore::set_checked`] echo.
fn reconstruct(conn: &rusqlite::Connection, resource: &str) -> Policy {
    let bindings = read_bindings(conn, resource);
    if bindings.is_empty() {
        return Policy::default();
    }
    let version = if bindings.iter().any(|b| b.condition.is_some()) {
        3
    } else {
        0
    };
    let mut policy = Policy {
        version,
        bindings,
        ..Policy::default()
    };
    policy.etag = aip::iam::policy::compute(&policy);
    policy
}

/// Open an in-memory SQLite database with the `iam_policy_bindings` table — the
/// decomposed IAM **Policy** store, one row per (resource, **Binding**, **Member**),
/// mirroring iam-go's `iamspanner` schema (aip #65). The primary key orders rows by
/// `(resource, binding_index, role, member_index, member)` so a policy reconstructs
/// in canonical order. Spanner's `STRING(MAX)`/`INT64` map to SQLite `TEXT`/
/// `INTEGER`. The nullable `condition` column holds the **Binding**'s **Condition**
/// expression (NULL ⇒ unconditional), so a stored conditional grant round-trips and
/// `TestIamPermissions` can evaluate it (#68); it looks up the **Policy** for the
/// requested **Resource name** directly, so no member-keyed reverse index is needed.
fn new_policies_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE iam_policy_bindings (
            resource      TEXT    NOT NULL,
            binding_index INTEGER NOT NULL,
            role          TEXT    NOT NULL,
            member_index  INTEGER NOT NULL,
            member        TEXT    NOT NULL,
            condition     TEXT,
            PRIMARY KEY (resource, binding_index, role, member_index, member)
        );",
    )
    .expect("create iam_policy_bindings table");
    conn
}

/// Reconstruct a resource's **Bindings** from `iam_policy_bindings`, in canonical
/// order. Rows are grouped by `binding_index` (each carries one **Role** and, when
/// present, one **Condition**); the `ORDER BY` makes the grouping contiguous and the
/// **Members** deterministic. The `condition` column is the same across a binding's
/// member rows, so it is read off whichever row first opens the binding and lifted
/// back into a [`google.type.Expr`](Expr) carrying just its expression (#68).
fn read_bindings(conn: &rusqlite::Connection, resource: &str) -> Vec<Binding> {
    let mut statement = conn
        .prepare(
            "SELECT binding_index, role, member, condition FROM iam_policy_bindings \
             WHERE resource = ?1 ORDER BY binding_index, member_index",
        )
        .expect("prepare policy read");
    let rows = statement
        .query_map([resource], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .expect("query policy bindings");

    let mut bindings: Vec<Binding> = Vec::new();
    let mut current: Option<i64> = None;
    for row in rows {
        let (binding_index, role, member, condition) = row.expect("read policy binding row");
        if current != Some(binding_index) {
            bindings.push(Binding {
                role,
                members: Vec::new(),
                condition: condition.map(|expression| Expr {
                    expression,
                    ..Expr::default()
                }),
            });
            current = Some(binding_index);
        }
        bindings
            .last_mut()
            .expect("a binding was just pushed")
            .members
            .push(member);
    }
    bindings
}

/// Map an aip-sql bind [`Value`](aip::sql::Value) onto rusqlite's owned value type
/// for positional binding.
fn to_sql(value: aip::sql::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        aip::sql::Value::Null => Sql::Null,
        aip::sql::Value::Bool(b) => Sql::Integer(b.into()),
        aip::sql::Value::Int(i) => Sql::Integer(i),
        aip::sql::Value::Double(d) => Sql::Real(d),
        aip::sql::Value::Text(s) => Sql::Text(s),
        aip::sql::Value::Bytes(b) => Sql::Blob(b),
    }
}
