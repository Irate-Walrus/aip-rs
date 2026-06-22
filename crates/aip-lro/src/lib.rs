//! AIP-151 long-running operations: the [`Operation`] state machine and its
//! AIP-193 errors, as a parse/validate primitive.
//!
//! A method too slow to answer inline returns a `google.longrunning.Operation` —
//! a promise the client polls until it is done (AIP-151). This crate owns the
//! *state* layer of that promise and nothing else: the three-state, terminal-once
//! [`State`] machine, the [`OperationName`] grammar, the [`WaitOperation`
//! timeout](WaitTimeout) policy, and the `OPERATION_*` [`Error`] shaping. Running
//! the work, persisting the [`Operation`], minting names, and serving the
//! `Operations` service stay the caller's — the same line `aip-filtering`
//! (ADR-0003), `aip-iam` (ADR-0010), and the database-agnostic core (ADR-0005)
//! all draw. See [`docs/adr/0015`](https://github.com/irate-walrus/aip-rs).
//!
//! # The Typed facade and the Dynamic core
//!
//! An `Operation` carries its **metadata** (progress) and its **response**
//! (success payload) as `google.protobuf.Any`. Packing a *typed* message into an
//! `Any` needs that message's type URL, which only
//! [`prost_reflect::ReflectMessage`] carries — a bare prost message does not know
//! its own `full_name`. So the headline [`Operation<M, R>`] is generic over the
//! metadata `M` and response `R` messages: it packs and unpacks the `Any` fields
//! so a caller never hand-writes `type.googleapis.com/…` (ADR-0009). It layers on
//! the still-public [`dynamic`] core, which runs the same transitions over
//! already-packed [`Any`](prost_types::Any) values — the JSON/gateway escape
//! hatch and the test surface.
//!
//! # State, not execution
//!
//! ```
//! use aip_lro::{Operation, OperationName, State};
//! use aip_proto::google::longrunning::{OperationInfo, GetOperationRequest};
//!
//! // Two stand-in typed messages (any `ReflectMessage` works as metadata/response).
//! let name = OperationName::parse("operations/import-42").unwrap();
//! let metadata = OperationInfo { response_type: "Shipper".into(), metadata_type: "Meta".into() };
//!
//! let mut op = Operation::<OperationInfo, GetOperationRequest>::pending(&name, &metadata);
//! assert_eq!(op.state(), State::Pending);
//!
//! // ... the caller's task does the work, then resolves the promise ...
//! let response = GetOperationRequest { name: "shippers/acme".into() };
//! op.succeed(&response).unwrap();
//! assert_eq!(op.state(), State::Succeeded);
//! assert_eq!(op.response().unwrap(), Some(response));
//!
//! // Terminal-once: a done operation never re-opens.
//! assert!(op.succeed(&GetOperationRequest::default()).is_err());
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use prost_reflect::ReflectMessage;
use prost_types::Any;
use tonic_types::Status;

use aip_proto::google::longrunning::{operation::Result as OpResult, Operation as ProtoOperation};

/// The gRPC status code for a cancelled operation (`google.rpc.Code.CANCELLED`),
/// the terminal error a [`cancel`](Operation::cancel) lands in.
const CANCELLED_CODE: i32 = 1;

/// Errors produced by the long-running operation state rules and name grammar.
///
/// Some variants are produced internally (a malformed [`OperationName`], a
/// terminal transition on a done operation); the rest — [`not_found`](Error::not_found)
/// and [`aborted`](Error::aborted) — are constructors a server raises from its
/// own handler (a store miss, a denied parallel operation), exactly as
/// `aip-iam` ships the AIP-211 NOT_FOUND helper without owning storage.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// An [`OperationName`] did not parse: it is not a valid resource name ending
    /// in `operations/{id}`. Maps to `INVALID_ARGUMENT`.
    #[error("operation name `{name}` is invalid: {detail}")]
    NameInvalid {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        detail: String,
    },
    /// No operation exists for the requested name — the handler's store missed.
    /// The name rides in `ErrorInfo.metadata`. Maps to `NOT_FOUND`.
    #[error("operation `{name}` not found")]
    NotFound {
        /// The requested operation name.
        name: String,
    },
    /// A terminal transition ([`succeed`](Operation::succeed) /
    /// [`fail`](Operation::fail) / [`cancel`](Operation::cancel)) or a
    /// [`set_metadata`](Operation::set_metadata) was attempted on an operation
    /// that is already done. An operation is terminal-once. Maps to
    /// `FAILED_PRECONDITION`.
    #[error("operation `{name}` is already done")]
    AlreadyDone {
        /// The operation name (empty for a validate-only operation).
        name: String,
    },
    /// A stored `Operation` violates the state invariant — done with no result,
    /// not done with a result, or no name — or its `metadata`/`response` `Any`
    /// failed to decode into the expected type. A data-integrity fault, not
    /// client input. Maps to `INTERNAL`.
    #[error("operation is malformed: {detail}")]
    Malformed {
        /// What was wrong with the stored operation.
        detail: String,
    },
    /// A `metadata`/`response` `Any` carried a different type than `M`/`R`
    /// expected. Maps to `INTERNAL` (a stored-data fault), with both type names
    /// in `ErrorInfo.metadata`.
    #[error("expected a `{expected}` but the operation carried a `{got}`")]
    TypeMismatch {
        /// The fully-qualified message name `M`/`R` expected.
        expected: String,
        /// The fully-qualified message name the `Any` actually carried.
        got: String,
    },
    /// A `WaitOperation` request carried a negative `timeout`. Maps to
    /// `INVALID_ARGUMENT`.
    #[error("wait timeout is invalid: {detail}")]
    WaitTimeoutInvalid {
        /// Why the timeout was rejected.
        detail: String,
    },
    /// A new operation was denied because one is already running on the resource
    /// and the resource does not permit parallel operations (AIP-151). Raised by
    /// a server from its own parallel-operation policy. Maps to `ABORTED`.
    #[error("operation `{name}` aborted: a conflicting operation is in progress")]
    Aborted {
        /// The resource (or operation) name the conflict is on.
        name: String,
    },
}

impl Error {
    /// The AIP-211-shaped not-found a handler raises when its store has no
    /// operation for `name` (`NOT_FOUND` on the wire). The crate owns no store;
    /// this is the reusable error shaping, the name carried for `ErrorInfo`.
    pub fn not_found(name: impl Into<String>) -> Self {
        Error::NotFound { name: name.into() }
    }

    /// The AIP-151 parallel-operation rejection a server raises when its policy
    /// denies a second operation on `name` (`ABORTED` on the wire). Deciding
    /// *when* to deny needs the store, so the policy is the caller's; this shapes
    /// the error.
    pub fn aborted(name: impl Into<String>) -> Self {
        Error::Aborted { name: name.into() }
    }
}

/// The parsed name of an [`Operation`]: an optional parent resource name, the
/// `operations` collection, and a caller-minted operation id (AIP-151).
///
/// Flat (`operations/{op}`) is the AIP-151 default; parent-scoped
/// (`workspaces/{w}/operations/{op}`) serves multi-tenant servers. The crate
/// *validates* a name but never *mints* the id — there is no clock or RNG in the
/// core (the same refusal `aip-events` makes for event ids); the caller supplies
/// the id and this checks its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationName {
    parent: Option<String>,
    operation_id: String,
}

impl OperationName {
    /// Parse `name` as `[{parent}/]operations/{operation}`.
    ///
    /// # Errors
    ///
    /// [`Error::NameInvalid`] when `name` is not a valid resource name, does not
    /// end in an `operations/{id}` pair, or the trailing id is not a valid
    /// resource id.
    pub fn parse(name: &str) -> Result<Self, Error> {
        let invalid = |detail: &str| Error::NameInvalid {
            name: name.to_owned(),
            detail: detail.to_owned(),
        };
        aip_resourcename::validate(name).map_err(|e| invalid(&e.to_string()))?;

        let (parent, id) = match name.rsplit_once("/operations/") {
            Some((parent, id)) => (Some(parent), id),
            None => match name.strip_prefix("operations/") {
                Some(id) => (None, id),
                None => return Err(invalid("missing the `operations/` collection")),
            },
        };
        if id.is_empty() || id.contains('/') {
            return Err(invalid("the operation id must be a single non-empty segment"));
        }
        aip_resourceid::validate_user_settable(id).map_err(|e| invalid(&e.to_string()))?;

        Ok(OperationName {
            parent: parent.map(str::to_owned),
            operation_id: id.to_owned(),
        })
    }

    /// The parent resource name (`workspaces/{w}`), or `None` for a flat name.
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    /// The trailing operation id — the resource id after `operations/`.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// The operations *collection* this name lives in (`{parent}/operations` or
    /// `operations`) — the value a `ListOperations` request carries in `name`.
    pub fn collection(&self) -> String {
        match &self.parent {
            Some(parent) => format!("{parent}/operations"),
            None => "operations".to_owned(),
        }
    }
}

impl fmt::Display for OperationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.parent {
            Some(parent) => write!(f, "{parent}/operations/{}", self.operation_id),
            None => write!(f, "operations/{}", self.operation_id),
        }
    }
}

impl FromStr for OperationName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OperationName::parse(s)
    }
}

/// Which of three states an [`Operation`] is in, derived from `done` and which
/// result field is set (the LRO analog of `aip-softdelete`'s `State`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Not done; carries no result. The work is still running.
    Pending,
    /// Done with a response — the operation succeeded.
    Succeeded,
    /// Done with an error — the operation failed (a cancellation lands here too).
    Failed,
}

impl State {
    /// The state of a raw proto operation: `Pending` while not done, `Succeeded`
    /// when done with a response, `Failed` when done otherwise. Total: a done
    /// operation that is not a response is `Failed`.
    fn of(op: &ProtoOperation) -> State {
        if !op.done {
            State::Pending
        } else if matches!(op.result, Some(OpResult::Response(_))) {
            State::Succeeded
        } else {
            State::Failed
        }
    }

    /// Whether the operation is done (`Succeeded` or `Failed`).
    pub fn is_done(self) -> bool {
        !matches!(self, State::Pending)
    }
}

/// The low-level operation surface over already-packed [`Any`] values — the
/// Dynamic core the [`Operation<M, R>`] Typed facade layers on (ADR-0009).
///
/// A caller holding a typed message uses the facade; a JSON/gateway caller that
/// already holds an `Any`, and the crate's own tests, use these functions. Every
/// transition guards "must be pending" so the terminal-once rule holds at both
/// surfaces.
pub mod dynamic {
    use super::{Error, OpResult, ProtoOperation, State, CANCELLED_CODE};
    use prost_types::Any;
    use tonic_types::Status;

    /// A new pending operation named `name` carrying `metadata`.
    pub fn pending(name: impl Into<String>, metadata: Option<Any>) -> ProtoOperation {
        ProtoOperation {
            name: name.into(),
            metadata,
            done: false,
            result: None,
        }
    }

    /// Resolve `op` to succeeded with the packed `response`.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if `op` is already done.
    pub fn succeed(op: &mut ProtoOperation, response: Any) -> Result<(), Error> {
        ensure_pending(op)?;
        op.done = true;
        op.result = Some(OpResult::Response(response));
        Ok(())
    }

    /// Resolve `op` to failed with `status`.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if `op` is already done.
    pub fn fail(op: &mut ProtoOperation, status: Status) -> Result<(), Error> {
        ensure_pending(op)?;
        op.done = true;
        op.result = Some(OpResult::Error(status));
        Ok(())
    }

    /// Resolve `op` to failed with a `CANCELLED` status (AIP-151 cancellation).
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if `op` is already done.
    pub fn cancel(op: &mut ProtoOperation) -> Result<(), Error> {
        fail(op, cancelled_status())
    }

    /// Replace a pending operation's `metadata` (progress).
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if `op` is already done.
    pub fn set_metadata(op: &mut ProtoOperation, metadata: Any) -> Result<(), Error> {
        ensure_pending(op)?;
        op.metadata = Some(metadata);
        Ok(())
    }

    /// The [`State`] of a raw operation.
    pub fn state(op: &ProtoOperation) -> State {
        State::of(op)
    }

    /// Check that a *stored* operation conforms to the state invariant: it has a
    /// name, and `done` holds exactly when exactly one result field is set.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] when the operation has no name, is done with no
    /// result, or is not done yet carries one.
    pub fn validate(op: &ProtoOperation) -> Result<(), Error> {
        if op.name.is_empty() {
            return Err(Error::Malformed {
                detail: "a stored operation must have a name".to_owned(),
            });
        }
        match (op.done, &op.result) {
            (false, None) | (true, Some(_)) => Ok(()),
            (true, None) => Err(Error::Malformed {
                detail: "done but carries no result".to_owned(),
            }),
            (false, Some(_)) => Err(Error::Malformed {
                detail: "not done but carries a result".to_owned(),
            }),
        }
    }

    /// The terminal `CANCELLED` `google.rpc.Status`.
    pub(super) fn cancelled_status() -> Status {
        Status {
            code: CANCELLED_CODE,
            message: "operation cancelled".to_owned(),
            details: Vec::new(),
        }
    }

    fn ensure_pending(op: &ProtoOperation) -> Result<(), Error> {
        if op.done {
            Err(Error::AlreadyDone {
                name: op.name.clone(),
            })
        } else {
            Ok(())
        }
    }
}

/// A typed view over a `google.longrunning.Operation`, generic over its metadata
/// message `M` and response message `R` (ADR-0009 / ADR-0015).
///
/// The facade packs `M`/`R` into the `metadata`/`response` `Any` fields on the
/// way in and unpacks them on the way out, so a caller works in typed messages
/// and never touches a type URL. It layers on the [`dynamic`] core. Transitions
/// are terminal-once.
pub struct Operation<M, R> {
    inner: ProtoOperation,
    // Invariant over the packed types without owning a value of them. `fn() -> T`
    // keeps the marker covariant and `Send`/`Sync` regardless of `M`/`R`.
    _types: std::marker::PhantomData<fn() -> (M, R)>,
}

impl<M, R> Operation<M, R> {
    /// The state of this operation.
    pub fn state(&self) -> State {
        dynamic::state(&self.inner)
    }

    /// The operation name (empty for a validate-only operation).
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The failure status, if this operation [`Failed`](State::Failed).
    pub fn error(&self) -> Option<&Status> {
        match &self.inner.result {
            Some(OpResult::Error(status)) => Some(status),
            _ => None,
        }
    }

    /// Resolve this operation to failed with `status`.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if it is already done.
    pub fn fail(&mut self, status: Status) -> Result<(), Error> {
        dynamic::fail(&mut self.inner, status)
    }

    /// Resolve this operation to failed with a `CANCELLED` status — the terminal
    /// state an AIP-151 cancellation lands in. Whether a cancel was *requested*
    /// is the caller's execution state; this is the resulting transition.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if it is already done.
    pub fn cancel(&mut self) -> Result<(), Error> {
        dynamic::cancel(&mut self.inner)
    }

    /// Borrow the underlying wire message (to serve or persist it).
    pub fn as_inner(&self) -> &ProtoOperation {
        &self.inner
    }

    /// Consume the facade for the underlying `google.longrunning.Operation` — the
    /// value a caller's store persists, keyed by [`name`](Operation::name).
    pub fn into_inner(self) -> ProtoOperation {
        self.inner
    }

    /// Rebuild a facade over a stored `google.longrunning.Operation`.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] when `inner` violates the state invariant (see
    /// [`dynamic::validate`]). A validate-only operation (empty name) is never
    /// stored, so it is rejected here.
    pub fn from_inner(inner: ProtoOperation) -> Result<Self, Error> {
        dynamic::validate(&inner)?;
        Ok(Operation {
            inner,
            _types: std::marker::PhantomData,
        })
    }

    /// Resolve this operation to failed, packing a `tonic::Status` into the
    /// `Operation.error` field — the convenience a caller uses to drop an
    /// aip-`Error` (already `Into<tonic::Status>`) into a failed operation
    /// without hand-building a `google.rpc.Status`.
    ///
    /// The status's code and message are forwarded; rich error details (the
    /// `grpc-status-details-bin` payload) are not — `Operation.error` carries the
    /// canonical code/message, which is what a poll surfaces.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if it is already done.
    #[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
    #[cfg(feature = "tonic")]
    pub fn fail_status(&mut self, status: tonic::Status) -> Result<(), Error> {
        self.fail(Status {
            code: status.code() as i32,
            message: status.message().to_owned(),
            details: Vec::new(),
        })
    }
}

impl<M: ReflectMessage + Default, R> Operation<M, R> {
    /// A new pending operation named `name` carrying the typed `metadata`.
    pub fn pending(name: &OperationName, metadata: &M) -> Self {
        Operation {
            inner: dynamic::pending(name.to_string(), Some(pack(metadata))),
            _types: std::marker::PhantomData,
        }
    }

    /// Replace this pending operation's progress `metadata`.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if it is already done.
    pub fn set_metadata(&mut self, metadata: &M) -> Result<(), Error> {
        dynamic::set_metadata(&mut self.inner, pack(metadata))
    }

    /// The typed metadata, unpacked from the `metadata` `Any`. `None` when the
    /// operation carries none (a validate-only operation).
    ///
    /// # Errors
    ///
    /// [`Error::TypeMismatch`] / [`Error::Malformed`] if the `Any` is not an `M`.
    pub fn metadata(&self) -> Result<Option<M>, Error> {
        self.inner.metadata.as_ref().map(unpack).transpose()
    }
}

impl<M, R: ReflectMessage + Default> Operation<M, R> {
    /// A done operation with the typed `response` and an empty name — the AIP-163
    /// validate-only reply, the one path to an empty name (a server keeps no
    /// state for a trivial validation). [`pending`](Operation::pending) and
    /// [`from_inner`](Operation::from_inner) keep the name required.
    pub fn validated(response: &R) -> Self {
        Operation {
            inner: ProtoOperation {
                name: String::new(),
                metadata: None,
                done: true,
                result: Some(OpResult::Response(pack(response))),
            },
            _types: std::marker::PhantomData,
        }
    }

    /// Resolve this operation to succeeded with the typed `response`.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyDone`] if it is already done.
    pub fn succeed(&mut self, response: &R) -> Result<(), Error> {
        dynamic::succeed(&mut self.inner, pack(response))
    }

    /// The typed response, unpacked from the `response` `Any`. `None` unless the
    /// operation [`Succeeded`](State::Succeeded).
    ///
    /// # Errors
    ///
    /// [`Error::TypeMismatch`] / [`Error::Malformed`] if the `Any` is not an `R`.
    pub fn response(&self) -> Result<Option<R>, Error> {
        match &self.inner.result {
            Some(OpResult::Response(any)) => Ok(Some(unpack(any)?)),
            _ => Ok(None),
        }
    }
}

/// Pack a typed message into an `Any`, deriving the type URL from its own
/// descriptor (ADR-0009) — the bit a caller would otherwise hand-roll.
fn pack<T: ReflectMessage>(msg: &T) -> Any {
    Any {
        type_url: format!("type.googleapis.com/{}", msg.descriptor().full_name()),
        value: msg.encode_to_vec(),
    }
}

/// Unpack an `Any` into the expected typed message, checking the type URL first.
fn unpack<T: ReflectMessage + Default>(any: &Any) -> Result<T, Error> {
    let expected = T::default().descriptor().full_name().to_owned();
    let got = any
        .type_url
        .rsplit_once('/')
        .map_or(any.type_url.as_str(), |(_, name)| name);
    if got != expected {
        return Err(Error::TypeMismatch {
            expected,
            got: got.to_owned(),
        });
    }
    T::decode(&any.value[..]).map_err(|e| Error::Malformed {
        detail: format!("decoding a `{expected}`: {e}"),
    })
}

/// The `WaitOperation` deadline policy: the default the server picks when the
/// client leaves `timeout` unset, and the max it caps a client's `timeout` to —
/// the LRO analog of AIP-158 size limits. Resolving is reflection- and
/// runtime-free; the *blocking* is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTimeout {
    default: Duration,
    max: Duration,
}

impl WaitTimeout {
    /// A policy with the given `default` (used when a request leaves `timeout`
    /// unset) and `max` (the ceiling a request's `timeout` is capped to).
    pub const fn new(default: Duration, max: Duration) -> Self {
        WaitTimeout { default, max }
    }

    /// Resolve a `WaitOperationRequest.timeout` to the duration the server should
    /// block: the [default](WaitTimeout::new) when `requested` is `None`, else the
    /// request's value capped at the [max](WaitTimeout::new). An explicit zero is
    /// honored (return immediately).
    ///
    /// # Errors
    ///
    /// [`Error::WaitTimeoutInvalid`] when `requested` is negative.
    pub fn resolve(&self, requested: Option<prost_types::Duration>) -> Result<Duration, Error> {
        let Some(duration) = requested else {
            return Ok(self.default);
        };
        if duration.seconds < 0 || duration.nanos < 0 {
            return Err(Error::WaitTimeoutInvalid {
                detail: "a wait timeout must not be negative".to_owned(),
            });
        }
        let requested = Duration::new(duration.seconds as u64, duration.nanos as u32);
        Ok(requested.min(self.max))
    }
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps; the `aip-rs`
/// sentinel the `aip-errordomain` boundary layer rewrites to the serving domain
/// (ADR-0007). Reason codes are unique within it.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to a canonical gRPC code with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `OPERATION_*` `reason`, the `aip-rs` `domain`,
    /// and the error's dynamic values as `metadata`. Every value here is opaque or
    /// resource-name-shaped, so no `BadRequest` field violation is attached
    /// (matching `aip-softdelete` / `aip-etag`).
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (code, reason, metadata): (tonic::Code, &str, HashMap<String, String>) = match &err {
            Error::NameInvalid { name, .. } => (
                tonic::Code::InvalidArgument,
                "OPERATION_NAME_INVALID",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
            Error::NotFound { name } => (
                tonic::Code::NotFound,
                "OPERATION_NOT_FOUND",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
            Error::AlreadyDone { name } => (
                tonic::Code::FailedPrecondition,
                "OPERATION_ALREADY_DONE",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
            Error::Malformed { detail } => (
                tonic::Code::Internal,
                "OPERATION_MALFORMED",
                HashMap::from([("detail".to_owned(), detail.clone())]),
            ),
            Error::TypeMismatch { expected, got } => (
                tonic::Code::Internal,
                "OPERATION_MALFORMED",
                HashMap::from([
                    ("expected".to_owned(), expected.clone()),
                    ("got".to_owned(), got.clone()),
                ]),
            ),
            Error::WaitTimeoutInvalid { .. } => (
                tonic::Code::InvalidArgument,
                "OPERATION_WAIT_TIMEOUT_INVALID",
                HashMap::new(),
            ),
            Error::Aborted { name } => (
                tonic::Code::Aborted,
                "OPERATION_ABORTED",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(code, message, details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aip_proto::google::longrunning::{GetOperationRequest, OperationInfo};

    /// Two stand-in typed messages for metadata (`OperationInfo`) and response
    /// (`GetOperationRequest`) — any `ReflectMessage` works.
    type Op = Operation<OperationInfo, GetOperationRequest>;

    fn metadata() -> OperationInfo {
        OperationInfo {
            response_type: "Shipper".to_owned(),
            metadata_type: "Meta".to_owned(),
        }
    }

    fn response() -> GetOperationRequest {
        GetOperationRequest {
            name: "shippers/acme".to_owned(),
        }
    }

    fn name() -> OperationName {
        OperationName::parse("operations/import-42").unwrap()
    }

    #[test]
    fn operation_name_flat_and_parented() {
        let flat = OperationName::parse("operations/abc").unwrap();
        assert_eq!(flat.parent(), None);
        assert_eq!(flat.operation_id(), "abc");
        assert_eq!(flat.collection(), "operations");
        assert_eq!(flat.to_string(), "operations/abc");

        let scoped = OperationName::parse("workspaces/w1/operations/abc").unwrap();
        assert_eq!(scoped.parent(), Some("workspaces/w1"));
        assert_eq!(scoped.operation_id(), "abc");
        assert_eq!(scoped.collection(), "workspaces/w1/operations");
        assert_eq!(scoped.to_string(), "workspaces/w1/operations/abc");
    }

    #[test]
    fn operation_name_rejects_malformed() {
        // Not an `operations/{id}` name.
        assert!(matches!(
            OperationName::parse("shippers/acme"),
            Err(Error::NameInvalid { .. })
        ));
        // Empty trailing id.
        assert!(OperationName::parse("operations/").is_err());
    }

    #[test]
    fn pending_carries_typed_metadata() {
        let op = Op::pending(&name(), &metadata());
        assert_eq!(op.state(), State::Pending);
        assert_eq!(op.name(), "operations/import-42");
        assert_eq!(op.metadata().unwrap(), Some(metadata()));
        assert_eq!(op.response().unwrap(), None);
        assert_eq!(op.error(), None);
    }

    #[test]
    fn succeed_packs_typed_response_and_is_terminal() {
        let mut op = Op::pending(&name(), &metadata());
        op.succeed(&response()).unwrap();
        assert_eq!(op.state(), State::Succeeded);
        assert_eq!(op.response().unwrap(), Some(response()));
        // Terminal-once.
        assert_eq!(
            op.succeed(&response()),
            Err(Error::AlreadyDone {
                name: "operations/import-42".to_owned()
            })
        );
    }

    #[test]
    fn cancel_lands_in_failed_with_cancelled_status() {
        let mut op = Op::pending(&name(), &metadata());
        op.cancel().unwrap();
        assert_eq!(op.state(), State::Failed);
        let status = op.error().expect("a cancelled operation carries an error");
        assert_eq!(status.code, CANCELLED_CODE);
        // Terminal-once after cancel.
        assert!(op.cancel().is_err());
    }

    #[test]
    fn set_metadata_only_while_pending() {
        let mut op = Op::pending(&name(), &metadata());
        let progress = OperationInfo {
            response_type: "Shipper".to_owned(),
            metadata_type: "50%".to_owned(),
        };
        op.set_metadata(&progress).unwrap();
        assert_eq!(op.metadata().unwrap(), Some(progress));
        op.succeed(&response()).unwrap();
        assert!(op.set_metadata(&metadata()).is_err());
    }

    #[test]
    fn validated_is_done_with_empty_name() {
        let op = Op::validated(&response());
        assert_eq!(op.state(), State::Succeeded);
        assert_eq!(op.name(), "");
        assert_eq!(op.response().unwrap(), Some(response()));
    }

    #[test]
    fn into_from_inner_round_trips_a_stored_operation() {
        let mut op = Op::pending(&name(), &metadata());
        op.succeed(&response()).unwrap();
        let stored = op.into_inner();
        let rebuilt = Op::from_inner(stored).unwrap();
        assert_eq!(rebuilt.state(), State::Succeeded);
        assert_eq!(rebuilt.response().unwrap(), Some(response()));
    }

    #[test]
    fn from_inner_rejects_invariant_violations() {
        // Done with no result.
        let bad = ProtoOperation {
            name: "operations/x".to_owned(),
            metadata: None,
            done: true,
            result: None,
        };
        assert!(matches!(Op::from_inner(bad), Err(Error::Malformed { .. })));

        // A stored operation must have a name (a validate-only op is not stored).
        let unnamed = Op::validated(&response()).into_inner();
        assert!(matches!(Op::from_inner(unnamed), Err(Error::Malformed { .. })));
    }

    #[test]
    fn unpack_detects_a_type_mismatch() {
        // Pack an `OperationInfo` as metadata, then read it back as the wrong type
        // by viewing the same wire op through a facade expecting a different `M`.
        let stored = Op::pending(&name(), &metadata()).into_inner();
        let wrong = Operation::<GetOperationRequest, GetOperationRequest>::from_inner(stored).unwrap();
        assert!(matches!(
            wrong.metadata(),
            Err(Error::TypeMismatch { .. })
        ));
    }

    #[test]
    fn wait_timeout_default_cap_and_negative() {
        let policy = WaitTimeout::new(Duration::from_secs(10), Duration::from_secs(60));
        // Unset -> default.
        assert_eq!(policy.resolve(None).unwrap(), Duration::from_secs(10));
        // Capped at max.
        assert_eq!(
            policy
                .resolve(Some(prost_types::Duration { seconds: 600, nanos: 0 }))
                .unwrap(),
            Duration::from_secs(60)
        );
        // Under the cap, honored.
        assert_eq!(
            policy
                .resolve(Some(prost_types::Duration { seconds: 5, nanos: 0 }))
                .unwrap(),
            Duration::from_secs(5)
        );
        // Explicit zero -> immediate.
        assert_eq!(
            policy
                .resolve(Some(prost_types::Duration { seconds: 0, nanos: 0 }))
                .unwrap(),
            Duration::ZERO
        );
        // Negative -> error.
        assert!(matches!(
            policy.resolve(Some(prost_types::Duration { seconds: -1, nanos: 0 })),
            Err(Error::WaitTimeoutInvalid { .. })
        ));
    }
}
