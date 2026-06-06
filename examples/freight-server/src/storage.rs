//! In-memory datastore backing the demo.
//!
//! Deliberately database-agnostic (ADR-0005): just keyed maps with interior
//! mutability, so the gRPC layer — not the store — is what exercises the aip-rs
//! primitives. State lives for the life of the process and resets on restart.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::proto::einride::example::freight::v1::{Shipper, Site};

/// Process-lifetime, in-memory store. Keyed by resource name; a `BTreeMap` keeps
/// listings in a stable order, which the pagination/ordering work relies on (it
/// is the deterministic tie-break behind a stable `order_by` sort).
///
/// Shipments are added as their handlers are wired up.
#[derive(Default)]
pub struct Storage {
    shippers: Mutex<BTreeMap<String, Shipper>>,
    sites: Mutex<BTreeMap<String, Site>>,
}

impl Storage {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
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

    /// Every site, in resource-name order. The service layer filters by parent
    /// and applies the AIP-132 `order_by` sort on top of this stable order.
    pub fn list_sites(&self) -> Vec<Site> {
        self.sites.lock().unwrap().values().cloned().collect()
    }

    /// Insert or overwrite a site, keyed by its `name`.
    pub fn put_site(&self, site: Site) {
        self.sites.lock().unwrap().insert(site.name.clone(), site);
    }
}
