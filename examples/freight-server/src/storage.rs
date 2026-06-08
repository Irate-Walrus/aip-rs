//! Datastore backing the demo.
//!
//! Shippers live in an in-memory map (ADR-0005); the gRPC layer — not the store —
//! is what exercises those primitives. Sites are backed by an **in-memory SQLite
//! database** so an AIP-160 **Filter** travels end-to-end into a real SQL engine
//! (ADR-0008): this is the default store, no feature flag. State lives for the
//! life of the process and resets on restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use aip::sql::Dialect as _;
use prost::Message as _;

use crate::proto::einride::example::freight::v1::{site::State, Shipment, Shipper, Site};
use crate::proto::google::iam::v1::Policy;

/// Process-lifetime store. Shippers are a `BTreeMap` keyed by resource name,
/// which keeps listings in a stable order (the deterministic tie-break behind a
/// stable `order_by` sort). Sites and shipments live in SQLite tables — the
/// filterable/sortable columns plus the full resource as wire bytes — so an
/// AIP-160 **Filter** composed with the server's own predicates (parent scope,
/// soft delete) travels end-to-end into a real SQL engine (ADR-0008).
pub struct Storage {
    shippers: Mutex<BTreeMap<String, Shipper>>,
    sites: Mutex<rusqlite::Connection>,
    shipments: Mutex<rusqlite::Connection>,
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
        }
    }

    /// Fetch a shipper by resource name.
    pub fn get_shipper(&self, name: &str) -> Option<Shipper> {
        self.shippers.lock().unwrap().get(name).cloned()
    }

    /// Every shipper, in resource-name order.
    pub fn list_shippers(&self) -> Vec<Shipper> {
        self.shippers.lock().unwrap().values().cloned().collect()
    }

    /// Insert or overwrite a shipper, keyed by its `name`.
    pub fn put_shipper(&self, shipper: Shipper) {
        self.shippers
            .lock()
            .unwrap()
            .insert(shipper.name.clone(), shipper);
    }

    /// Remove a shipper by name, returning it if it existed.
    pub fn remove_shipper(&self, name: &str) -> Option<Shipper> {
        self.shippers.lock().unwrap().remove(name)
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
        let create_time = site.create_time.as_ref().map(rfc3339);
        let update_time = site.update_time.as_ref().map(rfc3339);
        let delete_time = site.delete_time.as_ref().map(rfc3339);
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
        let create_time = shipment.create_time.as_ref().map(rfc3339);
        let delete_time = shipment.delete_time.as_ref().map(rfc3339);
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
/// The query is `SELECT data FROM <table> WHERE <predicate> ORDER BY <order_by>
/// LIMIT <limit> OFFSET <offset>`: the predicate renders to parameterized SQL
/// through the SQLite [`Dialect`](aip::sql::Dialect) and its bind values are bound
/// positionally — never spliced into the SQL text (ADR-0005 / ADR-0008). The
/// `ORDER BY` columns come from the [`Schema`](aip::sql::Schema) allowlist and the
/// `LIMIT` / `OFFSET` are server-resolved integers, so both are rendered directly
/// (no binds) by [`aip::sql::render_order_by`] / [`aip::sql::render_limit_offset`].
/// `table` is a fixed string literal supplied by the store, never request input.
fn query_page(
    conn: &rusqlite::Connection,
    table: &str,
    predicate: &aip::sql::Predicate,
    order_by: &[aip::sql::Order],
    limit: u64,
    offset: u64,
) -> Vec<Vec<u8>> {
    let (where_sql, binds) = aip::sql::Sqlite.render(predicate);
    let params: Vec<rusqlite::types::Value> = binds.into_iter().map(to_sql).collect();
    let order_clause = if order_by.is_empty() {
        String::new()
    } else {
        format!("ORDER BY {} ", aip::sql::render_order_by(order_by))
    };
    let page_clause = aip::sql::render_limit_offset(limit, offset);
    let sql = format!("SELECT data FROM {table} WHERE {where_sql} {order_clause}{page_clause}");
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

/// Format a protobuf `Timestamp` as a canonical RFC 3339 UTC string at second
/// precision, so the stored `create_time` column sorts and compares
/// lexicographically the same way the RFC3339 literal a filter binds does (the
/// transpiler binds timestamps as text, #40). Only the non-negative range the
/// demo produces (`now()`) is handled; the civil date comes from Howard
/// Hinnant's `civil_from_days` algorithm.
fn rfc3339(ts: &prost_types::Timestamp) -> String {
    let secs = ts.seconds.max(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, minute, second) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Process-lifetime store of `google.iam.v1.Policy` keyed by **Resource name**,
/// backing the demo's `IAMPolicy` service (aip #64). A `Policy` attaches to the
/// resource it governs (a shipper / site / shipment name), so the resource name
/// is the key; state resets on restart, like the rest of [`Storage`].
///
/// Kept separate from [`Storage`] (which owns the freight resources) because the
/// tracer bullet only needs a Member to travel request → validate → store →
/// response; cross-referencing a policy against the resource it names is a later
/// slice (the AIP-211 NOT_FOUND-via-parent path, #67).
#[derive(Default)]
pub struct PolicyStore {
    policies: Mutex<BTreeMap<String, Policy>>,
}

impl PolicyStore {
    /// An empty policy store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy attached to `resource`, or `None` when none is set — the caller
    /// turns that into the empty `Policy` `GetIamPolicy` returns (AIP / IAM: an
    /// unset policy is not an error).
    pub fn get(&self, resource: &str) -> Option<Policy> {
        self.policies.lock().unwrap().get(resource).cloned()
    }

    /// Attach `policy` to `resource`, replacing any existing one (the IAM
    /// `SetIamPolicy` replace semantics).
    pub fn set(&self, resource: String, policy: Policy) {
        self.policies.lock().unwrap().insert(resource, policy);
    }
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
