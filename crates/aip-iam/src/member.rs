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
}
