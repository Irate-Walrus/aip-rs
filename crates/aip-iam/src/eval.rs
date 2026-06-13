//! The opt-in CEL-backed **Condition** evaluation adapter — behind the non-default
//! `eval` feature (ADR-0010).
//!
//! An IAM **Condition** ([`google.type.Expr`]) is a *general* CEL string over IAM's
//! own environment (`resource.*`, `request.time`, …) — **not** the AIP-160 subset,
//! so `aip-filtering`'s CEL bridge is deliberately not used here (it would reject
//! most real conditions). To *do* anything with a Condition beyond storing it, we
//! reach for the `cel` crate (the cel-rust project): [`Condition::compile`](crate::eval::Condition::compile) parses
//! the expression once, and [`Condition::evaluate`](crate::eval::Condition::evaluate) runs the compiled program
//! against a [`RequestContext`](crate::eval::RequestContext) to a `bool`.
//!
//! This is the *execution* layer, so — like `aip-sql`'s SQL transpilation
//! (ADR-0005/0008) — it lives strictly behind an opt-in feature and never in a
//! default build: a parse/validate-only user never compiles the `cel`
//! parser/runtime. The authorization **decision** that *calls* this adapter
//! (role→permission expansion, member matching) stays the caller's.
//!
//! A malformed expression is an [`Error::ConditionMalformed`], and an expression
//! that compiles but fails to produce a `bool` is an [`Error::ConditionEvaluation`]
//! — both *distinct from* a `false` result, so a caller never mistakes a broken
//! condition for one that simply did not hold.
//!
//! [`google.type.Expr`]: https://github.com/googleapis/googleapis/blob/master/google/type/expr.proto

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use cel::{Context, Program, Value};

use crate::Error;

/// A compiled IAM **Condition**: the CEL expression parsed once into a reusable
/// program, ready to [`evaluate`](Condition::evaluate) against many requests.
///
/// Build one with [`compile`](Condition::compile) — the cost of parsing the CEL
/// source is paid there, so a server that re-checks the same **Binding** on every
/// request compiles the Condition once and evaluates it per request. The original
/// expression is retained so an evaluation error can name the offending Condition.
#[derive(Debug)]
pub struct Condition {
    program: Program,
    expression: String,
}

impl Condition {
    /// Compile a **Condition** expression (general CEL) into a reusable program.
    ///
    /// # Errors
    ///
    /// [`Error::ConditionMalformed`] if `expression` is not valid CEL — a *parse*
    /// failure, surfaced as a clear error rather than silently treated as `false`.
    pub fn compile(expression: &str) -> Result<Self, Error> {
        let program = Program::compile(expression).map_err(|err| Error::ConditionMalformed {
            expression: expression.to_owned(),
            detail: err.to_string(),
        })?;
        Ok(Self {
            program,
            expression: expression.to_owned(),
        })
    }

    /// Evaluate the compiled **Condition** against `request` to a `bool`: does this
    /// Condition hold for this request?
    ///
    /// # Errors
    ///
    /// [`Error::ConditionEvaluation`] if the program fails at runtime (e.g. it
    /// references a variable the context does not supply) or produces a non-boolean
    /// value. This is distinct from `Ok(false)`: a Condition that *cannot* be
    /// decided is an error, never a silent denial.
    pub fn evaluate(&self, request: &RequestContext) -> Result<bool, Error> {
        match self.program.execute(&request.to_cel_context()) {
            Ok(Value::Bool(held)) => Ok(held),
            Ok(other) => Err(Error::ConditionEvaluation {
                expression: self.expression.clone(),
                detail: format!("expression produced a non-boolean value: {other:?}"),
            }),
            Err(err) => Err(Error::ConditionEvaluation {
                expression: self.expression.clone(),
                detail: err.to_string(),
            }),
        }
    }
}

/// The IAM-style variable environment a **Condition** is evaluated against: the
/// `resource.*` attributes the request targets and the `request.time` it arrived
/// at, the two halves of the environment AIP/IAM conditions reach for.
///
/// Built fluently — [`new`](RequestContext::new) stamps `request.time` with the
/// moment the context is built (what every real caller passes), and each setter
/// layers on `resource.*` attributes or overrides the time:
///
/// ```
/// # use aip_iam::eval::{Condition, RequestContext};
/// // `request.time` defaults to now, so a resource-only Condition needs no clock.
/// let ctx = RequestContext::new().resource("name", "shippers/acme");
/// let held = Condition::compile("resource.name == \"shippers/acme\"")
///     .unwrap()
///     .evaluate(&ctx)
///     .unwrap();
/// assert!(held);
/// ```
#[derive(Debug, Clone)]
pub struct RequestContext {
    resource: BTreeMap<String, String>,
    request_time: SystemTime,
}

impl Default for RequestContext {
    /// Same as [`new`](RequestContext::new) — `request.time` stamped with now, no
    /// `resource.*` attributes.
    fn default() -> Self {
        Self::new()
    }
}

impl RequestContext {
    /// A fresh request context with `request.time` set to **now** — the instant the
    /// context is built, which is what every real caller passes — and no
    /// `resource.*` attributes yet.
    ///
    /// A test that needs a fixed clock overrides the default via
    /// [`request_time`](RequestContext::request_time); production code that just
    /// wants "when the request arrived" takes the default and drops a line.
    pub fn new() -> Self {
        Self {
            resource: BTreeMap::new(),
            request_time: SystemTime::now(),
        }
    }

    /// Set a `resource.<key>` attribute (e.g. `name`, `type`, `service`) the
    /// Condition can read. Replaces any previous value for the same key.
    #[must_use]
    pub fn resource(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource.insert(key.into(), value.into());
        self
    }

    /// Override `request.time` — the instant the request arrived — which a Condition
    /// can compare against a CEL `timestamp(...)` literal (e.g. an expiry window).
    ///
    /// [`new`](RequestContext::new) already defaults this to now, so the override
    /// exists for deterministic tests that pin a fixed clock; production callers
    /// rarely need it.
    #[must_use]
    pub fn request_time(mut self, time: SystemTime) -> Self {
        self.request_time = time;
        self
    }

    /// Lower this environment into a CEL [`Context`], binding `resource` to a map of
    /// its string attributes and `request` to a map carrying the `time` timestamp.
    /// [`Context::default`] supplies the standard CEL functions — `timestamp(...)`,
    /// `size(...)`, the time accessors — a Condition builds on.
    fn to_cel_context(&self) -> Context<'static> {
        let mut context = Context::default();

        let resource: HashMap<String, Value> = self
            .resource
            .iter()
            .map(|(key, value)| (key.clone(), Value::from(value.clone())))
            .collect();
        context.add_variable_from_value("resource", resource);

        // `request.time` is a CEL timestamp value (a chrono instant), not a string,
        // so a `request.time < timestamp(...)` comparison type-checks. It is always
        // populated — `new` defaults it to now — so a Condition reaching for it
        // never hits an absent value.
        let datetime: chrono::DateTime<chrono::Utc> = self.request_time.into();
        let request =
            HashMap::from([("time".to_owned(), Value::Timestamp(datetime.fixed_offset()))]);
        context.add_variable_from_value("request", request);

        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A `SystemTime` `secs` after the Unix epoch — a fixed instant so the
    /// time-window tests are deterministic.
    fn at_epoch_plus(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn a_condition_compiles_once_and_evaluates_to_true() {
        let condition = Condition::compile("1 < 2").expect("valid CEL");
        assert_eq!(condition.evaluate(&RequestContext::new()), Ok(true));
    }

    #[test]
    fn a_condition_evaluates_to_false() {
        let condition = Condition::compile("1 > 2").expect("valid CEL");
        assert_eq!(condition.evaluate(&RequestContext::new()), Ok(false));
    }

    #[test]
    fn a_malformed_condition_is_an_error_distinct_from_false() {
        // A parse failure surfaces as a clear error, never a silent `false`.
        let err = Condition::compile("1 +").expect_err("not valid CEL");
        assert!(
            matches!(err, Error::ConditionMalformed { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn resource_attributes_are_in_scope() {
        let condition = Condition::compile("resource.name == \"shippers/acme\"").expect("valid");

        let hit = RequestContext::new().resource("name", "shippers/acme");
        assert_eq!(condition.evaluate(&hit), Ok(true));

        // The same compiled Condition re-used against a different request.
        let miss = RequestContext::new().resource("name", "shippers/other");
        assert_eq!(condition.evaluate(&miss), Ok(false));
    }

    #[test]
    fn request_time_is_in_scope_and_compares_against_a_timestamp() {
        let condition = Condition::compile("request.time < timestamp(\"2030-01-01T00:00:00Z\")")
            .expect("valid");

        // 2023-11-14 is before the window — the Condition holds.
        let before = RequestContext::new().request_time(at_epoch_plus(1_700_000_000));
        assert_eq!(condition.evaluate(&before), Ok(true));

        // 2033-05-18 is after it — the Condition does not hold.
        let after = RequestContext::new().request_time(at_epoch_plus(2_000_000_000));
        assert_eq!(condition.evaluate(&after), Ok(false));
    }

    #[test]
    fn a_non_boolean_condition_is_an_evaluation_error() {
        // A well-formed expression that yields a non-bool is an error, not `false`.
        let condition = Condition::compile("1 + 1").expect("valid CEL");
        let err = condition
            .evaluate(&RequestContext::new())
            .expect_err("non-boolean result");
        assert!(
            matches!(err, Error::ConditionEvaluation { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn referencing_an_unsupplied_variable_is_an_evaluation_error() {
        // A Condition reaching for a variable the context does not bind (only
        // `resource` and `request` are in scope) is an error, distinct from
        // `false` — the request could not be decided.
        let condition = Condition::compile("principal == \"user:alice@example.com\"")
            .expect("valid CEL — `principal` is just an unbound identifier");
        let err = condition
            .evaluate(&RequestContext::new())
            .expect_err("`principal` is not in scope");
        assert!(
            matches!(err, Error::ConditionEvaluation { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn request_time_defaults_to_now_so_an_expiry_window_decides_without_a_clock() {
        // `new()` stamps `request.time`, so a time-window Condition decides with no
        // explicit `request_time` — the override is only for pinning a fixed clock.
        let open = Condition::compile("request.time < timestamp(\"2999-01-01T00:00:00Z\")")
            .expect("valid");
        assert_eq!(open.evaluate(&RequestContext::new()), Ok(true));

        let closed = Condition::compile("request.time < timestamp(\"2000-01-01T00:00:00Z\")")
            .expect("valid");
        assert_eq!(closed.evaluate(&RequestContext::new()), Ok(false));
    }
}
