//! Structural read-modify-write ops over a [`google.iam.v1.Policy`](Policy).
//!
//! The toolkit a server runs between a `GetIamPolicy` and a `SetIamPolicy`: add or
//! remove a **Member** from the **Binding** for a **Role** ([`add_member`] /
//! [`remove_member`]), fold a **Policy** into a canonical form so two equal
//! policies compare equal ([`normalize`]), enforce the *conditions ⟹ version 3*
//! invariant ([`validate`]), and run the `etag` optimistic-concurrency check that
//! makes the read-modify-write cycle safe ([`compute_etag`] / [`check_etag`]).
//!
//! These are *structural* ops — they rearrange a **Policy**'s **Bindings**; they
//! never make an authorization **decision** (role→permission expansion, condition
//! evaluation), which stays the caller's, behind the opt-in `eval` adapter
//! (ADR-0010). All of them are pure functions over the proto structure, so they
//! ride the `iam-proto` feature alongside [`Policy`].

use prost::Message as _;

use crate::proto::{Binding, Policy};
use crate::{Error, Member, Role};

/// Add `member` to the unconditional **Binding** for `role`, creating the Binding
/// when none exists. Idempotent: a member already granted the role is left as-is.
///
/// "Unconditional" means the **Binding** with no **Condition** — a conditional
/// binding is a distinct grant keyed by its **Condition**, so this never disturbs
/// one. Pass the parsed [`Role`] / [`Member`] so the values are validated before
/// they reach the **Policy**; they are rendered back to their `google.iam.v1` text
/// form for storage.
///
/// The result is *not* normalised — call [`normalize`] before storing if you want
/// the canonical form (e.g. members sorted). See `docs/adr/0010-iam-primitives.md`.
pub fn add_member(policy: &mut Policy, role: &Role, member: &Member) {
    let role = role.to_string();
    let member = member.to_string();
    match policy
        .bindings
        .iter_mut()
        .find(|b| b.role == role && b.condition.is_none())
    {
        Some(binding) => {
            if !binding.members.contains(&member) {
                binding.members.push(member);
            }
        }
        None => policy.bindings.push(Binding {
            role,
            members: vec![member],
            condition: None,
        }),
    }
}

/// Remove `member` from the unconditional **Binding** for `role`, pruning the
/// **Binding** when it becomes empty. Idempotent: removing a member that is not
/// granted the role (or a role with no binding) is a no-op.
///
/// Like [`add_member`] this targets only the **Binding** with no **Condition**; a
/// conditional grant of the same **Role** is left untouched.
pub fn remove_member(policy: &mut Policy, role: &Role, member: &Member) {
    let role = role.to_string();
    let member = member.to_string();
    let Some(index) = policy
        .bindings
        .iter()
        .position(|b| b.role == role && b.condition.is_none())
    else {
        return;
    };
    let binding = &mut policy.bindings[index];
    binding.members.retain(|m| *m != member);
    if binding.members.is_empty() {
        policy.bindings.remove(index);
    }
}

/// Fold `policy` into a canonical form so that two policies granting the same
/// **Members** the same **Roles** compare equal regardless of input ordering.
///
/// - within each **Binding**, **Members** are sorted and de-duplicated;
/// - **Bindings** sharing a `(role, condition)` key are merged into one;
/// - **Bindings** left with no **Members** are dropped;
/// - **Bindings** are sorted by `(role, condition)`.
///
/// The `version` and `etag` are left untouched — they are policy metadata, not
/// part of the binding set's identity (and [`compute_etag`] derives the `etag`
/// from the normalised content anyway).
pub fn normalize(policy: &mut Policy) {
    let mut merged: Vec<Binding> = Vec::new();
    for mut binding in std::mem::take(&mut policy.bindings) {
        binding.members.sort();
        binding.members.dedup();
        if binding.members.is_empty() {
            continue;
        }
        match merged
            .iter_mut()
            .find(|b| b.role == binding.role && b.condition == binding.condition)
        {
            Some(existing) => {
                existing.members.append(&mut binding.members);
                existing.members.sort();
                existing.members.dedup();
            }
            None => merged.push(binding),
        }
    }
    merged.sort_by(|a, b| binding_key(a).cmp(&binding_key(b)));
    policy.bindings = merged;
}

/// A total-order sort key over a **Binding**'s identity `(role, condition)`. The
/// whole **Condition** participates so distinct conditional grants of one **Role**
/// order deterministically (`None` sorts before any `Some`).
fn binding_key(binding: &Binding) -> (&str, Option<(&str, &str, &str, &str)>) {
    (
        binding.role.as_str(),
        binding.condition.as_ref().map(|expr| {
            (
                expr.expression.as_str(),
                expr.title.as_str(),
                expr.description.as_str(),
                expr.location.as_str(),
            )
        }),
    )
}

/// Enforce the *conditions ⟹ version 3* invariant: if any **Binding** carries a
/// **Condition**, the **Policy** `version` must be `3` (IAM rejects a conditional
/// binding on an older schema). A policy with no conditions is accepted at any
/// version, and version `3` without conditions is fine — only the conditional case
/// is constrained (ADR-0010).
///
/// # Errors
///
/// [`Error::PolicyConditionRequiresVersion3`] when a conditional binding is present
/// but `version != 3`.
pub fn validate(policy: &Policy) -> Result<(), Error> {
    if policy.bindings.iter().any(|b| b.condition.is_some()) && policy.version != 3 {
        return Err(Error::PolicyConditionRequiresVersion3 {
            version: policy.version,
        });
    }
    Ok(())
}

/// The content `etag` of `policy`: a deterministic CRC32 digest of its content,
/// used as the optimistic-concurrency token for the read-modify-write cycle.
///
/// The `etag` field itself is excluded from the digest (it is zeroed before
/// hashing), so the value is a pure function of the policy's content — recomputing
/// it over a stored policy reproduces the token a prior call returned. Normalise
/// first if you want the `etag` to be invariant under binding reordering; a server
/// that stores the normalised form gets that for free.
///
/// The token is opaque: callers compare it for equality (see [`check_etag`]) and
/// never parse it. The digest is a CRC32 over the encoded policy rendered as
/// lowercase hex — the same content-digest idiom `aip-pagination`'s request
/// checksum uses (a clone to clear the excluded field, then `crc32fast::hash`).
///
/// This is the **same digest scheme** the general-purpose `aip-etag` primitive
/// (issue #93) applies to any resource via reflection. `aip-etag` additionally
/// excludes `OUTPUT_ONLY` fields, but `google.iam.v1.Policy` carries none, so the
/// two produce identical tokens for a Policy. This path stays a direct,
/// reflection-free implementation over the concrete [`Policy`] type (no
/// `prost-reflect`/`aip-etag` dependency on `aip-iam`; ADR-0001).
pub fn compute_etag(policy: &Policy) -> Vec<u8> {
    let mut policy = policy.clone();
    policy.etag = Vec::new();
    let digest = crc32fast::hash(&policy.encode_to_vec());
    format!("{digest:08x}").into_bytes()
}

/// Optimistic-concurrency check for the `SetIamPolicy` read-modify-write cycle:
/// decide whether a write carrying `supplied` (the request policy's `etag`) may
/// proceed against `current` (the policy presently stored, or `None` if unset).
///
/// An empty `supplied` is an unconditional write — the caller opted out of the
/// concurrency check, so it always proceeds. Otherwise `supplied` must equal the
/// [`compute_etag`] of `current` (or of the empty [`Policy`] when nothing is
/// stored); a mismatch means another writer intervened.
///
/// # Errors
///
/// [`Error::PolicyEtagMismatch`] when a non-empty `supplied` does not match the
/// current policy — the AIP / IAM contract maps this to `ABORTED`, telling the
/// caller to re-read and retry (ADR-0010).
pub fn check_etag(supplied: &[u8], current: Option<&Policy>) -> Result<(), Error> {
    if supplied.is_empty() {
        return Ok(());
    }
    let current_etag = match current {
        Some(policy) => compute_etag(policy),
        None => compute_etag(&Policy::default()),
    };
    if supplied == current_etag.as_slice() {
        Ok(())
    } else {
        Err(Error::PolicyEtagMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::google::r#type::Expr;

    fn binding(role: &str, members: &[&str]) -> Binding {
        Binding {
            role: role.to_owned(),
            members: members.iter().map(|m| (*m).to_owned()).collect(),
            condition: None,
        }
    }

    fn role(s: &str) -> Role {
        s.parse().expect("test role parses")
    }

    fn member(s: &str) -> Member {
        s.parse().expect("test member parses")
    }

    #[test]
    fn add_member_creates_a_binding_then_extends_it() {
        let mut policy = Policy::default();

        add_member(
            &mut policy,
            &role("roles/viewer"),
            &member("user:alice@example.com"),
        );
        assert_eq!(
            policy.bindings,
            vec![binding("roles/viewer", &["user:alice@example.com"])]
        );

        // A second member for the same role extends the existing Binding.
        add_member(
            &mut policy,
            &role("roles/viewer"),
            &member("group:ops@example.com"),
        );
        assert_eq!(
            policy.bindings,
            vec![binding(
                "roles/viewer",
                &["user:alice@example.com", "group:ops@example.com"]
            )]
        );
    }

    #[test]
    fn add_member_is_idempotent() {
        let mut policy = Policy::default();
        let r = role("roles/editor");
        let m = member("user:bob@example.com");
        add_member(&mut policy, &r, &m);
        add_member(&mut policy, &r, &m);
        assert_eq!(
            policy.bindings,
            vec![binding("roles/editor", &["user:bob@example.com"])]
        );
    }

    #[test]
    fn remove_member_prunes_an_emptied_binding() {
        let mut policy = Policy {
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        };
        remove_member(
            &mut policy,
            &role("roles/viewer"),
            &member("user:alice@example.com"),
        );
        assert!(policy.bindings.is_empty(), "the emptied Binding is pruned");
    }

    #[test]
    fn remove_member_keeps_other_members_and_is_idempotent() {
        let mut policy = Policy {
            bindings: vec![binding(
                "roles/viewer",
                &["user:alice@example.com", "group:ops@example.com"],
            )],
            ..Policy::default()
        };
        let r = role("roles/viewer");
        let m = member("user:alice@example.com");
        remove_member(&mut policy, &r, &m);
        // The other member survives; removing the absent member again is a no-op.
        remove_member(&mut policy, &r, &m);
        assert_eq!(
            policy.bindings,
            vec![binding("roles/viewer", &["group:ops@example.com"])]
        );
    }

    #[test]
    fn remove_member_leaves_a_conditional_binding_alone() {
        // add/remove target only the unconditional Binding for the role.
        let conditional = Binding {
            condition: Some(Expr {
                expression: "request.time < timestamp(\"2030-01-01T00:00:00Z\")".to_owned(),
                ..Expr::default()
            }),
            ..binding("roles/viewer", &["user:alice@example.com"])
        };
        let mut policy = Policy {
            bindings: vec![conditional.clone()],
            ..Policy::default()
        };
        remove_member(
            &mut policy,
            &role("roles/viewer"),
            &member("user:alice@example.com"),
        );
        assert_eq!(
            policy.bindings,
            vec![conditional],
            "conditional grant untouched"
        );
    }

    #[test]
    fn normalize_dedupes_and_orders_so_equal_policies_compare_equal() {
        let one = Policy {
            bindings: vec![
                binding("roles/editor", &["user:bob@example.com"]),
                binding(
                    "roles/viewer",
                    &["group:ops@example.com", "user:alice@example.com"],
                ),
                // A duplicate of the viewer binding with an overlapping member.
                binding(
                    "roles/viewer",
                    &["user:alice@example.com", "user:carol@example.com"],
                ),
                // An empty binding to be dropped.
                binding("roles/owner", &[]),
            ],
            ..Policy::default()
        };
        // The same grants supplied in a different order, pre-merged differently.
        let two = Policy {
            bindings: vec![
                binding(
                    "roles/viewer",
                    &[
                        "user:carol@example.com",
                        "group:ops@example.com",
                        "user:alice@example.com",
                    ],
                ),
                binding(
                    "roles/editor",
                    &["user:bob@example.com", "user:bob@example.com"],
                ),
            ],
            ..Policy::default()
        };

        let mut a = one;
        let mut b = two;
        normalize(&mut a);
        normalize(&mut b);
        assert_eq!(
            a, b,
            "two policies with the same grants normalise identically"
        );
        assert_eq!(
            a.bindings,
            vec![
                binding("roles/editor", &["user:bob@example.com"]),
                binding(
                    "roles/viewer",
                    &[
                        "group:ops@example.com",
                        "user:alice@example.com",
                        "user:carol@example.com"
                    ]
                ),
            ]
        );
    }

    #[test]
    fn validate_requires_version_3_for_a_conditional_binding() {
        let conditional = Binding {
            condition: Some(Expr {
                expression: "true".to_owned(),
                ..Expr::default()
            }),
            ..binding("roles/viewer", &["user:alice@example.com"])
        };
        let mut policy = Policy {
            version: 1,
            bindings: vec![conditional],
            ..Policy::default()
        };
        assert_eq!(
            validate(&policy),
            Err(Error::PolicyConditionRequiresVersion3 { version: 1 })
        );

        // Bumping to version 3 satisfies the invariant.
        policy.version = 3;
        assert_eq!(validate(&policy), Ok(()));
    }

    #[test]
    fn validate_accepts_an_unconditional_policy_at_any_version() {
        let policy = Policy {
            version: 1,
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        };
        assert_eq!(validate(&policy), Ok(()));
    }

    #[test]
    fn etag_ignores_the_etag_field_and_is_stable() {
        let policy = Policy {
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        };
        let etag = compute_etag(&policy);

        // The same content carrying a different etag yields the same token.
        let stamped = Policy {
            etag: b"stale".to_vec(),
            ..policy.clone()
        };
        assert_eq!(compute_etag(&stamped), etag);

        // Different content yields a different token.
        let other = Policy {
            bindings: vec![binding("roles/editor", &["user:bob@example.com"])],
            ..Policy::default()
        };
        assert_ne!(compute_etag(&other), etag);
    }

    #[test]
    fn check_etag_allows_an_empty_supplied_etag() {
        let current = Policy {
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        };
        // An unconditional write opts out of the concurrency check.
        assert_eq!(check_etag(b"", Some(&current)), Ok(()));
    }

    #[test]
    fn check_etag_matches_the_current_policy_and_rejects_a_stale_one() {
        let current = Policy {
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        };
        let fresh = compute_etag(&current);
        assert_eq!(check_etag(&fresh, Some(&current)), Ok(()));

        // A token computed before a concurrent write no longer matches.
        let stale = compute_etag(&Policy::default());
        assert_eq!(
            check_etag(&stale, Some(&current)),
            Err(Error::PolicyEtagMismatch)
        );
    }

    #[test]
    fn check_etag_rejects_a_supplied_etag_when_nothing_is_stored() {
        // Supplying an etag for an unset policy means the caller expected a version
        // that does not exist — a conflict.
        let stale = compute_etag(&Policy {
            bindings: vec![binding("roles/viewer", &["user:alice@example.com"])],
            ..Policy::default()
        });
        assert_eq!(check_etag(&stale, None), Err(Error::PolicyEtagMismatch));
    }
}
