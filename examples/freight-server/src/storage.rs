//! Datastore backing the demo.
//!
//! Shippers, sites, and shipments live in one in-memory SQLite database as
//! typed-key tables: a resource name's variables are the key columns, every field
//! is its own typed column (small repeated/map fields ride as JSON), and a child's
//! parent key is a foreign key that cascades on hard delete. A filter composed with
//! the server's own predicates travels end-to-end into the engine. IAM **Policies**
//! are a separate SQLite store ([`PolicyStore`]). State lives for the life of the
//! process and resets on restart.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::proto::einride::example::freight::v1::{
    site::State, LineItem, Shipment, ShipmentResourceName, Shipper, ShipperResourceName, Site,
    SiteResourceName,
};
use crate::proto::google::longrunning::Operation;
// `Policy` / `Binding` are the generated `google.iam.v1` types the `aip::iam`
// helpers operate on and the `IAMPolicy` service speaks, so there is one `Policy`
// type. `Expr` is the `google.type.Expr` condition a `Binding` carries.
use aip::iam::proto::{google::r#type::Expr, Binding, Policy};
use aip_proto::google::r#type::LatLng;

/// The `shippers` columns, in a fixed order shared by every shipper read.
const SHIPPER_COLUMNS: &str = "shipper, display_name, create_time, update_time, delete_time, etag";

/// The `sites` columns, in a fixed order shared by every site read.
const SITE_COLUMNS: &str = "shipper, site, display_name, create_time, update_time, \
     delete_time, latitude, longitude, state, annotations, tags";

/// The `shipments` columns, in a fixed order shared by every shipment read.
const SHIPMENT_COLUMNS: &str = "shipper, shipment, create_time, update_time, delete_time, \
     origin_site, destination_site, pickup_earliest_time, pickup_latest_time, \
     delivery_earliest_time, delivery_latest_time, line_items, annotations, external_reference_id";

/// Process-lifetime store. Shippers, sites, and shipments are typed-key tables in
/// one SQLite connection, so a filter composed with the server's parent scope and
/// soft-delete predicate runs in a real SQL engine and a hard-deleted shipper
/// cascades to its children.
pub struct Storage {
    db: Mutex<rusqlite::Connection>,
    /// Idempotency cache: per `request_id`, the wire bytes of the create request
    /// that first used it and of the response it produced. The lookup and the
    /// record are two separate lock acquisitions, so two concurrent creates sharing
    /// a brand-new id can both miss and proceed — the demo does not reserve an id
    /// across the whole handler.
    ///
    /// [`Replay`]: aip::requestid::Replay
    idempotency: Mutex<BTreeMap<String, IdempotentRecord>>,
}

/// One idempotency-cache entry: the wire bytes of the create request that first
/// used a `request_id` and of the response it produced. The `request` bytes tell an
/// identical replay from a conflicting reuse; the `response` bytes decode back into
/// the resource on a replay.
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
    /// An empty store with a fresh in-memory SQLite database.
    pub fn new() -> Self {
        Self {
            db: Mutex::new(new_db()),
            idempotency: Mutex::new(BTreeMap::new()),
        }
    }

    /// The record stored the first time `request_id` was used on a create, or
    /// `None` if it is unseen. The caller compares the stored request against the
    /// incoming one to tell an identical replay from a conflicting reuse.
    pub fn idempotent_get(&self, request_id: &str) -> Option<IdempotentRecord> {
        self.idempotency.lock().unwrap().get(request_id).cloned()
    }

    /// Record a create's request + response wire bytes under `request_id`, so a
    /// later retry with the same id replays the response instead of acting again.
    pub fn idempotent_put(&self, request_id: String, request: Vec<u8>, response: Vec<u8>) {
        self.idempotency
            .lock()
            .unwrap()
            .insert(request_id, IdempotentRecord { request, response });
    }

    // ----- Shippers -----

    /// Fetch a shipper by its typed name.
    pub fn get_shipper(&self, name: &ShipperResourceName) -> Option<Shipper> {
        let conn = self.db.lock().unwrap();
        conn.query_row(
            &format!("SELECT {SHIPPER_COLUMNS} FROM shippers WHERE shipper = ?1"),
            [name.shipper()],
            row_to_shipper,
        )
        .optional()
        .expect("get shipper")
    }

    /// One page of shippers matching `predicate`, ordered and seeked in SQL. The
    /// predicate carries the soft-delete filter and the cursor seek; the order is
    /// the key tie-break.
    pub fn list_shippers_page(
        &self,
        predicate: &aip::sql::Predicate,
        order_by: &[aip::sql::Order],
        limit: u64,
    ) -> Vec<Shipper> {
        let conn = self.db.lock().unwrap();
        query_page(
            &conn,
            "shippers",
            SHIPPER_COLUMNS,
            predicate,
            order_by,
            limit,
            row_to_shipper,
        )
    }

    /// Insert or update a shipper, keyed by its typed name. An upsert (not a
    /// delete-and-reinsert) so updating or soft-deleting a shipper leaves its sites
    /// and shipments untouched — only a hard delete cascades.
    pub fn put_shipper(&self, name: &ShipperResourceName, shipper: Shipper) {
        let conn = self.db.lock().unwrap();
        conn.execute(
            "INSERT INTO shippers (shipper, display_name, create_time, update_time, delete_time, etag) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(shipper) DO UPDATE SET \
               display_name = excluded.display_name, create_time = excluded.create_time, \
               update_time = excluded.update_time, delete_time = excluded.delete_time, \
               etag = excluded.etag",
            rusqlite::params![
                name.shipper(),
                shipper.display_name,
                shipper.create_time.as_ref().map(store_timestamp),
                shipper.update_time.as_ref().map(store_timestamp),
                shipper.delete_time.as_ref().map(store_timestamp),
                shipper.etag,
            ],
        )
        .expect("upsert shipper");
    }

    // ----- Sites -----

    /// Fetch a site by its typed name (a primary-key lookup on its key columns).
    pub fn get_site(&self, name: &SiteResourceName) -> Option<Site> {
        let conn = self.db.lock().unwrap();
        conn.query_row(
            &format!("SELECT {SITE_COLUMNS} FROM sites WHERE shipper = ?1 AND site = ?2"),
            [name.shipper(), name.site()],
            row_to_site,
        )
        .optional()
        .expect("get site")
    }

    /// One page of sites matching `predicate`, ordered and seeked in SQL. The
    /// predicate is the server's composed `WHERE` — the parent scope, the soft-delete
    /// filter, any user filter, and the cursor seek.
    pub fn list_sites_page(
        &self,
        predicate: &aip::sql::Predicate,
        order_by: &[aip::sql::Order],
        limit: u64,
    ) -> Vec<Site> {
        let conn = self.db.lock().unwrap();
        query_page(
            &conn,
            "sites",
            SITE_COLUMNS,
            predicate,
            order_by,
            limit,
            row_to_site,
        )
    }

    /// Insert or update a site, keyed by its typed name. The name decomposes into
    /// the key binds; the rest of the message becomes typed columns, with the map
    /// and list fields stored as JSON.
    pub fn put_site(&self, name: &SiteResourceName, site: Site) {
        let latitude = site.lat_lng.as_ref().map(|ll| ll.latitude);
        let longitude = site.lat_lng.as_ref().map(|ll| ll.longitude);
        let state = State::try_from(site.state)
            .unwrap_or(State::Unspecified)
            .as_str_name();
        let annotations = serde_json::to_string(&site.annotations).expect("serialize annotations");
        let tags = serde_json::to_string(&site.tags).expect("serialize tags");
        let conn = self.db.lock().unwrap();
        conn.execute(
            "INSERT INTO sites (shipper, site, display_name, create_time, update_time, \
               delete_time, latitude, longitude, state, annotations, tags) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(shipper, site) DO UPDATE SET \
               display_name = excluded.display_name, create_time = excluded.create_time, \
               update_time = excluded.update_time, delete_time = excluded.delete_time, \
               latitude = excluded.latitude, longitude = excluded.longitude, \
               state = excluded.state, annotations = excluded.annotations, tags = excluded.tags",
            rusqlite::params![
                name.shipper(),
                name.site(),
                site.display_name,
                site.create_time.as_ref().map(store_timestamp),
                site.update_time.as_ref().map(store_timestamp),
                site.delete_time.as_ref().map(store_timestamp),
                latitude,
                longitude,
                state,
                annotations,
                tags,
            ],
        )
        .expect("upsert site");
    }

    // ----- Shipments -----

    /// One page of shipments matching `predicate`, ordered and seeked in SQL — the
    /// same composed-`WHERE` path as [`list_sites_page`](Storage::list_sites_page).
    pub fn list_shipments_page(
        &self,
        predicate: &aip::sql::Predicate,
        order_by: &[aip::sql::Order],
        limit: u64,
    ) -> Vec<Shipment> {
        let conn = self.db.lock().unwrap();
        query_page(
            &conn,
            "shipments",
            SHIPMENT_COLUMNS,
            predicate,
            order_by,
            limit,
            row_to_shipment,
        )
    }

    /// Insert or update a shipment, keyed by its typed name. The repeated
    /// `line_items` and the `annotations` map are stored as JSON; every other field
    /// is its own typed column.
    pub fn put_shipment(&self, name: &ShipmentResourceName, shipment: Shipment) {
        let line_items = serde_json::to_string(
            &shipment
                .line_items
                .iter()
                .map(LineItemJson::from_proto)
                .collect::<Vec<_>>(),
        )
        .expect("serialize line items");
        let annotations =
            serde_json::to_string(&shipment.annotations).expect("serialize annotations");
        let conn = self.db.lock().unwrap();
        conn.execute(
            "INSERT INTO shipments (shipper, shipment, create_time, update_time, delete_time, \
               origin_site, destination_site, pickup_earliest_time, pickup_latest_time, \
               delivery_earliest_time, delivery_latest_time, line_items, annotations, \
               external_reference_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
             ON CONFLICT(shipper, shipment) DO UPDATE SET \
               create_time = excluded.create_time, update_time = excluded.update_time, \
               delete_time = excluded.delete_time, origin_site = excluded.origin_site, \
               destination_site = excluded.destination_site, \
               pickup_earliest_time = excluded.pickup_earliest_time, \
               pickup_latest_time = excluded.pickup_latest_time, \
               delivery_earliest_time = excluded.delivery_earliest_time, \
               delivery_latest_time = excluded.delivery_latest_time, \
               line_items = excluded.line_items, annotations = excluded.annotations, \
               external_reference_id = excluded.external_reference_id",
            rusqlite::params![
                name.shipper(),
                name.shipment(),
                shipment.create_time.as_ref().map(store_timestamp),
                shipment.update_time.as_ref().map(store_timestamp),
                shipment.delete_time.as_ref().map(store_timestamp),
                shipment.origin_site,
                shipment.destination_site,
                shipment.pickup_earliest_time.as_ref().map(store_timestamp),
                shipment.pickup_latest_time.as_ref().map(store_timestamp),
                shipment
                    .delivery_earliest_time
                    .as_ref()
                    .map(store_timestamp),
                shipment.delivery_latest_time.as_ref().map(store_timestamp),
                line_items,
                annotations,
                shipment.external_reference_id,
            ],
        )
        .expect("upsert shipment");
    }
}

#[cfg(test)]
impl Storage {
    /// Hard-delete a shipper, cascading to its sites and shipments through the
    /// foreign key. Returns whether a row was removed. The `DeleteShipper` RPC
    /// soft-deletes, so the cascade is exercised only here.
    pub(crate) fn hard_delete_shipper(&self, name: &ShipperResourceName) -> bool {
        let conn = self.db.lock().unwrap();
        conn.execute("DELETE FROM shippers WHERE shipper = ?1", [name.shipper()])
            .expect("hard-delete shipper")
            > 0
    }

    /// Every stored shipper regardless of soft-delete state, in key order — a test
    /// convenience for asserting how many rows exist.
    pub(crate) fn list_shippers(&self) -> Vec<Shipper> {
        let conn = self.db.lock().unwrap();
        let mut statement = conn
            .prepare(&format!(
                "SELECT {SHIPPER_COLUMNS} FROM shippers ORDER BY shipper"
            ))
            .expect("prepare shipper list");
        let rows = statement
            .query_map([], row_to_shipper)
            .expect("query shippers");
        rows.map(|row| row.expect("read shipper row")).collect()
    }
}

/// Run one ordered, seeked, limited list query against `table`, mapping each row
/// through `to_row`. The store owns only the `SELECT <columns> FROM <table>` head;
/// the composed `predicate` renders to a parameterized `WHERE` whose binds are
/// bound positionally, and the ordered columns plus the `LIMIT` carry no binds.
/// `table` and `columns` are fixed literals, never request input.
fn query_page<T>(
    conn: &rusqlite::Connection,
    table: &str,
    columns: &str,
    predicate: &aip::sql::Predicate,
    order_by: &[aip::sql::Order],
    limit: u64,
    to_row: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Vec<T> {
    let (tail, binds) = aip::sql::Query::new()
        .filter(predicate.clone())
        .order_by(order_by.iter().cloned())
        .limit(limit)
        .render(&aip::sql::Sqlite);
    let params: Vec<rusqlite::types::Value> = binds.into_iter().map(to_sql).collect();
    let sql = format!("SELECT {columns} FROM {table} {tail}");
    let mut statement = conn.prepare(&sql).expect("prepare list query");
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| to_row(row))
        .expect("run list query");
    rows.map(|row| row.expect("read list row")).collect()
}

/// Assemble a [`Shipper`] from its columns, reconstructing the resource name from
/// the key column.
fn row_to_shipper(row: &rusqlite::Row) -> rusqlite::Result<Shipper> {
    let shipper: String = row.get("shipper")?;
    let name = ShipperResourceName::new(&shipper)
        .expect("a stored shipper key is a valid name")
        .to_string();
    Ok(Shipper {
        name,
        display_name: row.get("display_name")?,
        create_time: load_timestamp(row.get("create_time")?),
        update_time: load_timestamp(row.get("update_time")?),
        delete_time: load_timestamp(row.get("delete_time")?),
        etag: row.get("etag")?,
    })
}

/// Assemble a [`Site`] from its columns, reconstructing the resource name from the
/// key columns and the JSON map/list fields.
fn row_to_site(row: &rusqlite::Row) -> rusqlite::Result<Site> {
    let shipper: String = row.get("shipper")?;
    let site: String = row.get("site")?;
    let name = SiteResourceName::new(&shipper, &site)
        .expect("a stored site key is a valid name")
        .to_string();
    let latitude: Option<f64> = row.get("latitude")?;
    let longitude: Option<f64> = row.get("longitude")?;
    let state_name: String = row.get("state")?;
    let annotations: String = row.get("annotations")?;
    let tags: String = row.get("tags")?;
    Ok(Site {
        name,
        create_time: load_timestamp(row.get("create_time")?),
        update_time: load_timestamp(row.get("update_time")?),
        delete_time: load_timestamp(row.get("delete_time")?),
        display_name: row.get("display_name")?,
        lat_lng: lat_lng(latitude, longitude),
        state: State::from_str_name(&state_name).unwrap_or(State::Unspecified) as i32,
        annotations: serde_json::from_str(&annotations).expect("deserialize annotations"),
        tags: serde_json::from_str(&tags).expect("deserialize tags"),
    })
}

/// Assemble a [`Shipment`] from its columns, reconstructing the resource name from
/// the key columns and the JSON `line_items` / `annotations`.
fn row_to_shipment(row: &rusqlite::Row) -> rusqlite::Result<Shipment> {
    let shipper: String = row.get("shipper")?;
    let shipment: String = row.get("shipment")?;
    let name = ShipmentResourceName::new(&shipper, &shipment)
        .expect("a stored shipment key is a valid name")
        .to_string();
    let line_items: String = row.get("line_items")?;
    let line_items: Vec<LineItemJson> =
        serde_json::from_str(&line_items).expect("deserialize line items");
    let annotations: String = row.get("annotations")?;
    Ok(Shipment {
        name,
        create_time: load_timestamp(row.get("create_time")?),
        update_time: load_timestamp(row.get("update_time")?),
        delete_time: load_timestamp(row.get("delete_time")?),
        origin_site: row.get("origin_site")?,
        destination_site: row.get("destination_site")?,
        pickup_earliest_time: load_timestamp(row.get("pickup_earliest_time")?),
        pickup_latest_time: load_timestamp(row.get("pickup_latest_time")?),
        delivery_earliest_time: load_timestamp(row.get("delivery_earliest_time")?),
        delivery_latest_time: load_timestamp(row.get("delivery_latest_time")?),
        line_items: line_items
            .into_iter()
            .map(LineItemJson::into_proto)
            .collect(),
        annotations: serde_json::from_str(&annotations).expect("deserialize annotations"),
        external_reference_id: row.get("external_reference_id")?,
    })
}

/// A line item as stored in the JSON `line_items` column — the generated message
/// carries no serde derive, so this mirror carries the JSON shape.
#[derive(Serialize, Deserialize)]
struct LineItemJson {
    title: String,
    quantity: f32,
    weight_kg: f32,
    volume_m3: f32,
    external_reference_id: String,
}

impl LineItemJson {
    /// The JSON view of a proto line item.
    fn from_proto(item: &LineItem) -> Self {
        Self {
            title: item.title.clone(),
            quantity: item.quantity,
            weight_kg: item.weight_kg,
            volume_m3: item.volume_m3,
            external_reference_id: item.external_reference_id.clone(),
        }
    }

    /// Back to the proto line item.
    fn into_proto(self) -> LineItem {
        LineItem {
            title: self.title,
            quantity: self.quantity,
            weight_kg: self.weight_kg,
            volume_m3: self.volume_m3,
            external_reference_id: self.external_reference_id,
        }
    }
}

/// Rebuild a [`LatLng`] from its flattened columns; absent when both are NULL.
fn lat_lng(latitude: Option<f64>, longitude: Option<f64>) -> Option<LatLng> {
    match (latitude, longitude) {
        (None, None) => None,
        (latitude, longitude) => Some(LatLng {
            latitude: latitude.unwrap_or_default(),
            longitude: longitude.unwrap_or_default(),
        }),
    }
}

/// Render a timestamp as the canonical second-precision RFC3339 text the store
/// holds. Using the library formatter keeps a stored value comparable with the
/// second-precision literal a filter binds, and makes lexicographic text order
/// match chronological order. Server-stamped times carry no sub-second part (see
/// `now`), so the round-trip is exact.
pub(crate) fn store_timestamp(ts: &prost_types::Timestamp) -> String {
    aip::sql::format_timestamp(ts)
}

/// Parse a stored RFC3339 timestamp back into a protobuf timestamp.
fn load_timestamp(text: Option<String>) -> Option<prost_types::Timestamp> {
    text.map(|text| text.parse().expect("a stored timestamp parses"))
}

/// Open the in-memory SQLite database holding the typed-key `shippers`, `sites`,
/// and `shipments` tables. Foreign keys are enabled per connection so a hard-deleted
/// shipper cascades to its children, and `sites` carries a covering index over the
/// `display_name` listing.
fn new_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    // SQLite leaves foreign keys off per connection, so turn them on before the DDL.
    conn.pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    conn.execute_batch(
        "CREATE TABLE shippers (
            shipper      TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            create_time  TEXT,
            update_time  TEXT,
            delete_time  TEXT,
            etag         TEXT NOT NULL
        );
        CREATE TABLE sites (
            shipper      TEXT NOT NULL,
            site         TEXT NOT NULL,
            display_name TEXT NOT NULL,
            create_time  TEXT,
            update_time  TEXT,
            delete_time  TEXT,
            latitude     REAL,
            longitude    REAL,
            state        TEXT NOT NULL,
            annotations  TEXT NOT NULL,
            tags         TEXT NOT NULL,
            PRIMARY KEY (shipper, site),
            FOREIGN KEY (shipper) REFERENCES shippers(shipper) ON DELETE CASCADE
        );
        CREATE INDEX sites_by_display_name ON sites (shipper, display_name, site);
        CREATE TABLE shipments (
            shipper                TEXT NOT NULL,
            shipment               TEXT NOT NULL,
            create_time            TEXT,
            update_time            TEXT,
            delete_time            TEXT,
            origin_site            TEXT NOT NULL,
            destination_site       TEXT NOT NULL,
            pickup_earliest_time   TEXT,
            pickup_latest_time     TEXT,
            delivery_earliest_time TEXT,
            delivery_latest_time   TEXT,
            line_items             TEXT NOT NULL,
            annotations            TEXT NOT NULL,
            external_reference_id  TEXT NOT NULL,
            PRIMARY KEY (shipper, shipment),
            FOREIGN KEY (shipper) REFERENCES shippers(shipper) ON DELETE CASCADE
        );",
    )
    .expect("create freight tables");
    conn
}

/// Process-lifetime store of `google.iam.v1.Policy`, backing the demo's
/// `IAMPolicy` service. Like a real IAM backend, a **Policy** is stored *decomposed*
/// into the `iam_policy_bindings` table — one row per (resource, **Binding**,
/// **Member**) — rather than as an opaque blob; the **Policy** is reconstructed on
/// read and its `etag` is a content digest computed from that canonical form.
///
/// Each row also carries its **Binding**'s **Condition** expression (the
/// `condition` column, NULL for an unconditional binding) so that the stored
/// **Policy** round-trips its **Conditions** and `TestIamPermissions` can evaluate
/// them through the opt-in `eval` adapter. The policy `version` is *not* stored — it
/// is reconstructed from the *conditions ⟹ version 3* invariant ([`reconstruct`]):
/// version 3 when any binding is conditional, else version 1's default. The
/// invariant itself is still enforced by [`aip::iam::policy::validate`] in the
/// handler *before* a write, so a conditional binding on an old version is rejected
/// up front. A **Condition**'s `title` / `description` are not persisted (only the
/// `expression` the adapter needs). State lives for the process and resets on
/// restart, like the rest of [`Storage`].
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
    /// turns that into the empty `Policy` `GetIamPolicy` returns (an unset policy is
    /// not an error). The reconstructed policy carries the content `etag`
    /// ([`aip::iam::policy::compute`]), so a client can round-trip it back as its
    /// read-modify-write token.
    pub fn get(&self, resource: &str) -> Option<Policy> {
        let conn = self.bindings.lock().unwrap();
        let policy = reconstruct(&conn, resource);
        (!policy.bindings.is_empty()).then_some(policy)
    }

    /// Apply `SetIamPolicy` read-modify-write semantics atomically, the way a real
    /// IAM backend does it inside a transaction.
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
        // row first reconstructs it on read.
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
    // One library function owns version recovery, so this store does not hand-roll
    // it and the recovered `version` always agrees with what `policy::validate`
    // enforces on the write path.
    let version = aip::iam::policy::canonical_version(&bindings);
    let mut policy = Policy {
        version,
        bindings,
        ..Policy::default()
    };
    policy.etag = aip::iam::policy::compute(&policy);
    policy
}

/// Open an in-memory SQLite database with the `iam_policy_bindings` table — the
/// decomposed IAM **Policy** store, one row per (resource, **Binding**, **Member**).
/// The primary key orders rows by `(resource, binding_index, role, member_index,
/// member)` so a policy reconstructs in canonical order. The nullable `condition`
/// column holds the **Binding**'s **Condition** expression (NULL ⇒ unconditional),
/// so a stored conditional grant round-trips and `TestIamPermissions` can evaluate
/// it; it looks up the **Policy** for the requested **Resource name** directly, so no
/// member-keyed reverse index is needed.
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
/// back into a [`google.type.Expr`](Expr) carrying just its expression.
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

/// Process-lifetime store of long-running [`Operation`]s, backing the demo's
/// `google.longrunning.Operations` service and the `BatchCreateShippers` task.
/// `aip-lro` owns no store — storage is the caller's — so this is the freight-side
/// store, keyed by operation name; the stored value is the wire [`Operation`], kept
/// in memory.
///
/// Two things sit alongside, deliberately outside the library: the set of
/// cancel-requested names — caller execution state the batch task polls, since
/// "cancel asked, work winding down" has no wire field — and a [`Notify`] pulsed on
/// every change so `WaitOperation` blocks instead of busy-polling.
pub struct OperationStore {
    operations: Mutex<BTreeMap<String, Operation>>,
    cancels: Mutex<BTreeSet<String>>,
    changed: Notify,
}

impl Default for OperationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationStore {
    /// An empty operation store.
    pub fn new() -> Self {
        Self {
            operations: Mutex::new(BTreeMap::new()),
            cancels: Mutex::new(BTreeSet::new()),
            changed: Notify::new(),
        }
    }

    /// Insert or overwrite an operation, keyed by its own `name`, and wake any
    /// `WaitOperation` waiters. The batch task calls this after every transition
    /// (progress, success, cancellation).
    pub fn put(&self, operation: Operation) {
        self.operations
            .lock()
            .unwrap()
            .insert(operation.name.clone(), operation);
        self.changed.notify_waiters();
    }

    /// The operation named `name`, or `None` if the store has none.
    pub fn get(&self, name: &str) -> Option<Operation> {
        self.operations.lock().unwrap().get(name).cloned()
    }

    /// Every operation in `collection` (the `operations` collection a
    /// `ListOperations` names), in name order — those whose name sits directly under
    /// `{collection}/`.
    pub fn list(&self, collection: &str) -> Vec<Operation> {
        let prefix = format!("{collection}/");
        self.operations
            .lock()
            .unwrap()
            .values()
            .filter(|operation| operation.name.starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// Remove the operation named `name` (a `DeleteOperation`); returns whether it
    /// was present. Deletion drops the record — it does **not** cancel the work.
    pub fn remove(&self, name: &str) -> bool {
        let removed = self.operations.lock().unwrap().remove(name).is_some();
        // Drop any cancel flag alongside the record, so the flag's lifetime is tied
        // to the operation's rather than orphaned in the set after a delete.
        self.cancels.lock().unwrap().remove(name);
        if removed {
            self.changed.notify_waiters();
        }
        removed
    }

    /// Record that cancellation was requested for `name` (a best-effort
    /// `CancelOperation`). This is caller execution state: it does not flip `done`;
    /// the batch task notices the flag and lands the operation in `CANCELLED`.
    pub fn request_cancel(&self, name: &str) {
        self.cancels.lock().unwrap().insert(name.to_owned());
    }

    /// Clear a consumed cancel flag — the batch task calls this once it has observed
    /// the request and landed the operation in `CANCELLED`, so the flag does not
    /// linger on the now-terminal operation.
    pub fn clear_cancel(&self, name: &str) {
        self.cancels.lock().unwrap().remove(name);
    }

    /// Whether cancellation has been requested for `name`.
    pub fn is_cancel_requested(&self, name: &str) -> bool {
        self.cancels.lock().unwrap().contains(name)
    }

    /// A future that resolves the next time any operation changes. The concrete
    /// [`Notified`](tokio::sync::futures::Notified) is returned (not an opaque
    /// future) so `WaitOperation` can `enable()` it — enrol it in the waiter list
    /// *before* re-reading the operation, so a change landing between the read and
    /// the await cannot be missed (`tokio::sync::Notify` only enrols a `Notified` on
    /// poll otherwise).
    pub fn changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.changed.notified()
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
