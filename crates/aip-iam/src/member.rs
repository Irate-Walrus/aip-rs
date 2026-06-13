//! IAM **Members** — the principals a Policy **Binding** grants a **Role** to.

use std::fmt;
use std::str::FromStr;

use crate::Error;

/// A `google.iam.v1` member: an identity a Policy **Binding** grants a **Role** to.
///
/// Parsed from the `type:value` member grammar and rendered back by
/// [`Display`](fmt::Display) — the round-trip is lossless. The classic identity
/// set is modelled here; the `deleted:*` variants and the `principal:` /
/// `principalSet:` (workforce / workload identity federation) forms are deferred —
/// see `docs/adr/0010-iam-primitives.md`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Member {
    /// `allUsers` — anyone on the internet, authenticated or not.
    AllUsers,
    /// `allAuthenticatedUsers` — any authenticated principal.
    AllAuthenticatedUsers,
    /// `user:{email}` — a specific Google account.
    User(String),
    /// `serviceAccount:{email}` — a service account.
    ServiceAccount(String),
    /// `group:{email}` — a Google group.
    Group(String),
    /// `domain:{domain}` — every account in a Workspace domain.
    Domain(String),
}

impl FromStr for Member {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "" => Err(Error::MemberEmpty),
            "allUsers" => Ok(Member::AllUsers),
            "allAuthenticatedUsers" => Ok(Member::AllAuthenticatedUsers),
            _ => {
                let (kind, value) = s.split_once(':').ok_or_else(|| Error::MemberUnknownType {
                    prefix: s.to_owned(),
                })?;
                if value.is_empty() {
                    return Err(Error::MemberEmptyValue {
                        kind: kind.to_owned(),
                    });
                }
                let value = value.to_owned();
                match kind {
                    "user" => Ok(Member::User(value)),
                    "serviceAccount" => Ok(Member::ServiceAccount(value)),
                    "group" => Ok(Member::Group(value)),
                    "domain" => Ok(Member::Domain(value)),
                    other => Err(Error::MemberUnknownType {
                        prefix: other.to_owned(),
                    }),
                }
            }
        }
    }
}

impl Member {
    /// Does the stored Policy member string `granted` admit *this* **Member**?
    ///
    /// The two wildcard grants — `allUsers` and `allAuthenticatedUsers` — admit
    /// any present member (this method is only ever called with a concrete
    /// **Member** in hand, so "authenticated" is already satisfied); a typed grant
    /// admits only the **Member** whose canonical [`Display`](fmt::Display) form
    /// equals `granted`. Comparing against the canonical rendering is what makes
    /// the match exact — a malformed grant matches nothing.
    ///
    /// This is the per-grant half of the IAM membership rule; the anonymous
    /// caller (no **Member** at all) is handled by [`grant_admits`], and
    /// whole-**Policy** membership by [`policy::grants`](crate::policy::grants).
    /// The authorization **decision** (role→permission expansion, **Condition**
    /// evaluation) stays the caller's (ADR-0010).
    pub fn matches_grant(&self, granted: &str) -> bool {
        match granted {
            "allUsers" | "allAuthenticatedUsers" => true,
            typed => self.to_string() == typed,
        }
    }
}

/// Does the stored Policy member string `granted` admit `caller` (the request's
/// **Member**, or `None` for an anonymous caller)?
///
/// The full `google.iam.v1` membership rule for a single grant:
///
/// - `allUsers` admits anyone — even an absent (anonymous) caller;
/// - `allAuthenticatedUsers` admits any *present* caller, anonymous denied;
/// - a typed grant (`user:`, `group:`, …) admits only the exact canonical
///   [`Member`] (via [`Member::matches_grant`]).
///
/// This is the anonymous-aware companion to [`Member::matches_grant`]: reach for
/// the method when a concrete **Member** is in hand, this free function when the
/// caller may be anonymous (a server's AIP-211 gate). Coarse membership only —
/// the authorization **decision** stays the caller's (ADR-0010).
pub fn grant_admits(granted: &str, caller: Option<&Member>) -> bool {
    match (granted, caller) {
        // allUsers admits even an absent caller; every other grant needs one.
        ("allUsers", _) => true,
        (granted, Some(member)) => member.matches_grant(granted),
        (_, None) => false,
    }
}

impl fmt::Display for Member {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Member::AllUsers => f.write_str("allUsers"),
            Member::AllAuthenticatedUsers => f.write_str("allAuthenticatedUsers"),
            Member::User(email) => write!(f, "user:{email}"),
            Member::ServiceAccount(email) => write!(f, "serviceAccount:{email}"),
            Member::Group(email) => write!(f, "group:{email}"),
            Member::Domain(domain) => write!(f, "domain:{domain}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_recognised_form() {
        let forms = [
            "allUsers",
            "allAuthenticatedUsers",
            "user:alice@example.com",
            "serviceAccount:svc@project.iam.gserviceaccount.com",
            "group:admins@example.com",
            "domain:example.com",
        ];
        for form in forms {
            let member: Member = form.parse().expect("recognised member parses");
            assert_eq!(member.to_string(), form, "round-trip {form:?}");
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!("".parse::<Member>(), Err(Error::MemberEmpty));
    }

    #[test]
    fn rejects_unknown_type() {
        assert_eq!(
            "robot:r2d2".parse::<Member>(),
            Err(Error::MemberUnknownType {
                prefix: "robot".to_owned(),
            })
        );
        // No `:` at all is also an unknown type (the whole string is the prefix).
        assert_eq!(
            "alice@example.com".parse::<Member>(),
            Err(Error::MemberUnknownType {
                prefix: "alice@example.com".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_empty_value() {
        assert_eq!(
            "user:".parse::<Member>(),
            Err(Error::MemberEmptyValue {
                kind: "user".to_owned(),
            })
        );
    }

    #[test]
    fn matches_grant_honours_wildcards_and_exact_member() {
        let alice: Member = "user:alice@example.com".parse().unwrap();

        // A concrete member is admitted by either wildcard grant.
        assert!(alice.matches_grant("allUsers"));
        assert!(alice.matches_grant("allAuthenticatedUsers"));

        // A typed grant admits only the exact canonical rendering.
        assert!(alice.matches_grant("user:alice@example.com"));
        assert!(!alice.matches_grant("user:bob@example.com"));
        // Right value, wrong type is not a match.
        assert!(!alice.matches_grant("group:alice@example.com"));
    }

    #[test]
    fn grant_admits_handles_the_anonymous_caller() {
        let alice: Member = "user:alice@example.com".parse().unwrap();

        // allUsers admits anyone, including an absent (anonymous) caller.
        assert!(grant_admits("allUsers", None));
        assert!(grant_admits("allUsers", Some(&alice)));

        // allAuthenticatedUsers admits any present caller, but not the anonymous one.
        assert!(!grant_admits("allAuthenticatedUsers", None));
        assert!(grant_admits("allAuthenticatedUsers", Some(&alice)));

        // A typed grant needs a present caller matching it exactly.
        assert!(!grant_admits("user:alice@example.com", None));
        assert!(grant_admits("user:alice@example.com", Some(&alice)));
        assert!(!grant_admits("user:bob@example.com", Some(&alice)));
    }
}
