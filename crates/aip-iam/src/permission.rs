//! IAM **Permissions** — the `service.resource.verb` units a **Role** bundles.

use std::borrow::Cow;
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
///
/// Inner repr is `Cow<'static, str>`: [`from_static`](Permission::from_static)
/// borrows a literal (no alloc, `const`), [`FromStr`] owns a parsed string. Eq/Hash
/// run over the str, so a const-built value equals (and hashes like) a parsed one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Permission(Cow<'static, str>);

/// True if `s` matches `service.resource.verb`: ≥3 dot-separated segments, each a
/// letter then letters/digits. One `const fn` so [`Permission::from_static`] (panic)
/// and [`FromStr`] (Result) cannot diverge. Byte loop — no alloc, no split.
const fn is_well_formed(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    // completed segments + chars in current segment.
    let mut segments = 0;
    let mut seg_len = 0;
    while i < len {
        let b = bytes[i];
        if b == b'.' {
            // empty segment (leading/trailing/doubled dot) -> malformed.
            if seg_len == 0 {
                return false;
            }
            segments += 1;
            seg_len = 0;
        } else if seg_len == 0 {
            // first char of segment must be a letter.
            if !b.is_ascii_alphabetic() {
                return false;
            }
            seg_len += 1;
        } else if !b.is_ascii_alphanumeric() {
            // rest must be letters or digits.
            return false;
        } else {
            seg_len += 1;
        }
        i += 1;
    }
    // trailing segment must be non-empty; need ≥3 total.
    if seg_len == 0 {
        return false;
    }
    segments += 1;
    segments >= 3
}

impl Permission {
    /// Make `Permission` from `&'static str` literal at compile time.
    ///
    /// `const fn`, like [`http::HeaderValue::from_static`]. Bind to a `const` and a
    /// malformed literal fails the **build**:
    ///
    /// ```
    /// use aip_iam::Permission;
    /// const GET: Permission = Permission::from_static("freight.shippers.get");
    /// assert_eq!(GET.service(), "freight");
    /// assert_eq!(GET, "freight.shippers.get".parse().unwrap());
    /// ```
    ///
    /// # Panics
    ///
    /// Malformed literal -> panic (compile-time inside a `const`, else at the call).
    /// Message names the `service.resource.verb` rule; const panic carries no
    /// formatting, so it cannot echo the offending literal back.
    ///
    /// Want a runtime/fallible parse? Use [`FromStr`] (`str::parse`); it shares this
    /// exact validator, so the two never disagree. The `aip::iam::authz` denial
    /// helpers take `&Permission`, never `&str` — the type stays the proof of
    /// validity (ADR-0010).
    ///
    /// [`http::HeaderValue::from_static`]: https://docs.rs/http/latest/http/header/struct.HeaderValue.html#method.from_static
    pub const fn from_static(permission: &'static str) -> Permission {
        if !is_well_formed(permission) {
            panic!(
                "malformed Permission literal: want service.resource.verb \
                 (≥3 dot-separated segments, each a letter then letters/digits), \
                 e.g. \"freight.shippers.get\""
            );
        }
        Permission(Cow::Borrowed(permission))
    }

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
        // Same `const fn` validator `from_static` panics on — one rule, two paths.
        if is_well_formed(s) {
            Ok(Permission(Cow::Owned(s.to_owned())))
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
    fn from_static_equals_parsed() {
        // Const-built (Cow::Borrowed) and runtime-parsed (Cow::Owned) compare and
        // hash by the underlying str, so the two are interchangeable.
        const GET: Permission = Permission::from_static("freight.shippers.get");
        let parsed: Permission = "freight.shippers.get".parse().expect("well-formed");
        assert_eq!(GET, parsed);

        let mut set = std::collections::HashSet::new();
        set.insert(GET.clone());
        assert!(set.contains(&parsed), "const and parsed hash alike");

        // Accessors read the same off a const-built value.
        assert_eq!(GET.service(), "freight");
        assert_eq!(GET.verb(), "get");
        assert_eq!(GET.as_str(), "freight.shippers.get");
    }

    #[test]
    #[should_panic(expected = "service.resource.verb")]
    fn from_static_panics_on_malformed() {
        // Called at runtime, the panic path fires (a `const` binding would fail the
        // build instead). Message names the rule.
        let _ = Permission::from_static("freight.get");
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
