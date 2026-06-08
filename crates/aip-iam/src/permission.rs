//! IAM **Permissions** — the `service.resource.verb` units a **Role** bundles.

use std::fmt;
use std::str::FromStr;

use crate::Error;

/// A validated `google.iam.v1` permission of the form `service.resource.verb`
/// (e.g. `freight.shippers.get`, `iam.serviceAccounts.keys.create`).
///
/// At least three `.`-separated segments — a `service`, one or more `resource`
/// segments, and a `verb` — each a letter followed by letters or digits. The
/// original text is preserved, so [`Display`](fmt::Display) round-trips and
/// [`service`](Permission::service) / [`verb`](Permission::verb) read off the ends
/// without re-validating.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Permission(String);

impl Permission {
    /// The leading `service` segment.
    pub fn service(&self) -> &str {
        self.0
            .split('.')
            .next()
            .expect("validated: at least three segments")
    }

    /// The trailing `verb` segment.
    pub fn verb(&self) -> &str {
        self.0
            .rsplit('.')
            .next()
            .expect("validated: at least three segments")
    }

    /// The full `service.resource.verb` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Permission {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let segments: Vec<&str> = s.split('.').collect();
        let well_formed = segments.len() >= 3
            && segments.iter().all(|seg| {
                let mut chars = seg.chars();
                matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
                    && chars.all(|c| c.is_ascii_alphanumeric())
            });
        if well_formed {
            Ok(Permission(s.to_owned()))
        } else {
            Err(Error::PermissionMalformed {
                permission: s.to_owned(),
            })
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_exposes_service_and_verb() {
        let perm: Permission = "freight.shippers.get"
            .parse()
            .expect("well-formed permission");
        assert_eq!(perm.service(), "freight");
        assert_eq!(perm.verb(), "get");
        assert_eq!(perm.to_string(), "freight.shippers.get");

        // Multi-segment resource: service is the first, verb the last.
        let nested: Permission = "iam.serviceAccounts.keys.create"
            .parse()
            .expect("well-formed");
        assert_eq!(nested.service(), "iam");
        assert_eq!(nested.verb(), "create");
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "freight",
            "freight.get",
            "freight..get",
            "freight.1shippers.get",
            ".a.b",
        ] {
            assert_eq!(
                bad.parse::<Permission>(),
                Err(Error::PermissionMalformed {
                    permission: bad.to_owned(),
                }),
                "{bad:?} should be malformed"
            );
        }
    }
}
