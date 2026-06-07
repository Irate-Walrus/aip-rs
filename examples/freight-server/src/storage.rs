//! Datastore backing the demo.
//!
//! Shippers live in an in-memory map (ADR-0005); the gRPC layer — not the store —
//! is what exercises those primitives. Sites are backed by an **in-memory SQLite
//! database** so an AIP-160 **Filter** travels end-to-end into a real SQL engine
//! (ADR-0008): this is the default store, no feature flag. State lives for the
//! life of the process and resets on restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use aip_sql::Dialect as _;
use prost::Message as _;

use crate::proto::einride::example::freight::v1::{Shipper, Site};

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
    /// wire bytes alongside its filterable columns.
    pub fn put_site(&self, site: Site) {
        self.sites
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO sites (name, display_name, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![site.name, site.display_name, site.encode_to_vec()],
            )
            .expect("insert site");
    }

    /// Sites matching `predicate`, in resource-name order, read from SQLite. A
    /// `None` predicate returns every site. The predicate is rendered to
    /// parameterized SQL by the SQLite [`Dialect`](aip_sql::Dialect) and its bind
    /// values are passed as positional parameters — never spliced into the SQL
    /// text (ADR-0005 / ADR-0008). Parent scoping and ordering stay in the service
    /// layer this slice (`scope_to_parent` is #43; SQL `ORDER BY` is #42).
    pub fn list_sites_matching(&self, predicate: Option<&aip_sql::Predicate>) -> Vec<Site> {
        let conn = self.sites.lock().unwrap();
        let (sql, params) = match predicate {
            Some(predicate) => {
                let (where_sql, binds) = aip_sql::Sqlite.render(predicate);
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
/// primary key, the filterable `display_name` column, and the full site as wire
/// bytes for lossless round-trips.
fn new_sites_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE sites (
            name         TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            data         BLOB NOT NULL
        );",
    )
    .expect("create sites table");
    conn
}

/// Map an aip-sql bind [`Value`](aip_sql::Value) onto rusqlite's owned value type
/// for positional binding.
fn to_sql(value: aip_sql::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        aip_sql::Value::Null => Sql::Null,
        aip_sql::Value::Bool(b) => Sql::Integer(b.into()),
        aip_sql::Value::Int(i) => Sql::Integer(i),
        aip_sql::Value::Double(d) => Sql::Real(d),
        aip_sql::Value::Text(s) => Sql::Text(s),
        aip_sql::Value::Bytes(b) => Sql::Blob(b),
    }
}
