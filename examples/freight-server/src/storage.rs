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

use crate::proto::einride::example::freight::v1::{site::State, Shipper, Site};

/// Process-lifetime store. Shippers are a `BTreeMap` keyed by resource name,
/// which keeps listings in a stable order (the deterministic tie-break behind a
/// stable `order_by` sort). Sites live in a SQLite table — the filterable columns
/// plus the full site as wire bytes.
///
/// Shipments are added as their handlers are wired up.
pub struct Storage {
    shippers: Mutex<BTreeMap<String, Shipper>>,
    sites: Mutex<rusqlite::Connection>,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage {
    /// An empty store, with a fresh in-memory SQLite database for sites.
    pub fn new() -> Self {
        Self {
            shippers: Mutex::new(BTreeMap::new()),
            sites: Mutex::new(new_sites_db()),
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
    /// wire bytes alongside the columns an AIP-160 filter can address: the scalar
    /// `display_name`, the timestamp `create_time` as sortable RFC3339 text, the
    /// nested `lat_lng.latitude` flattened to a numeric column, and the enum
    /// `state` as its value name (matching the transpiler's enum rendering, #40).
    pub fn put_site(&self, site: Site) {
        let create_time = site.create_time.as_ref().map(rfc3339);
        let latitude = site.lat_lng.as_ref().map(|ll| ll.latitude);
        let state = State::try_from(site.state)
            .unwrap_or(State::Unspecified)
            .as_str_name();
        self.sites
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO sites \
                 (name, display_name, create_time, latitude, state, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    site.name,
                    site.display_name,
                    create_time,
                    latitude,
                    state,
                    site.encode_to_vec(),
                ],
            )
            .expect("insert site");
    }

    /// Sites matching `predicate`, in resource-name order, read from SQLite. A
    /// `None` predicate returns every site. The predicate is rendered to
    /// parameterized SQL by the SQLite [`Dialect`](aip::sql::Dialect) and its bind
    /// values are passed as positional parameters — never spliced into the SQL
    /// text (ADR-0005 / ADR-0008). Parent scoping and ordering stay in the service
    /// layer this slice (`scope_to_parent` is #43; SQL `ORDER BY` is #42).
    pub fn list_sites_matching(&self, predicate: Option<&aip::sql::Predicate>) -> Vec<Site> {
        let conn = self.sites.lock().unwrap();
        let (sql, params) = match predicate {
            Some(predicate) => {
                let (where_sql, binds) = aip::sql::Sqlite.render(predicate);
                let params: Vec<rusqlite::types::Value> = binds.into_iter().map(to_sql).collect();
                (
                    format!("SELECT data FROM sites WHERE {where_sql} ORDER BY name"),
                    params,
                )
            }
            None => (
                "SELECT data FROM sites ORDER BY name".to_string(),
                Vec::new(),
            ),
        };
        let mut statement = conn.prepare(&sql).expect("prepare site query");
        let rows = statement
            .query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .expect("run site query");
        rows.map(|data| Site::decode(data.expect("read site row").as_slice()).expect("decode site"))
            .collect()
    }
}

/// Open an in-memory SQLite database with the `sites` table: the resource name as
/// primary key, the filterable `display_name` / `create_time` / `latitude` /
/// `state` columns the AIP-160 filter addresses (#40), and the full site as wire
/// bytes for lossless round-trips.
fn new_sites_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE sites (
            name         TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            create_time  TEXT,
            latitude     REAL,
            state        TEXT NOT NULL,
            data         BLOB NOT NULL
        );",
    )
    .expect("create sites table");
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
