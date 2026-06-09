//! [`GrammaticalName`] — the singular or plural grammatical name of a resource
//! type. Ports aip-go's `reflect/aipreflect.GrammaticalName`.

use crate::{strcase::initial_upper_case, Error};

/// The grammatical name for the singular or plural form of a resource type,
/// e.g. `userEvent` / `userEvents`.
///
/// Grammatical names must be URL-safe and `lowerCamelCase`; [`validate`] checks
/// this. The accessors operate on whatever string was supplied even if it is
/// malformed (matching aip-go).
///
/// [`validate`]: Self::validate
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GrammaticalName(String);

impl GrammaticalName {
    /// Wraps a grammatical name string. Does not validate — call
    /// [`validate`](Self::validate) for that.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The grammatical name string, e.g. `userEvents`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks the name is non-empty, URL-safe (letters and digits only), and
    /// `lowerCamelCase` (starts with a lower-case letter). Ports aip-go's
    /// `Validate`.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |reason: String| Error::InvalidGrammaticalName {
            name: self.0.clone(),
            reason,
        };
        if self.0.is_empty() {
            return Err(invalid("must be non-empty".to_owned()));
        }
        for c in self.0.chars() {
            if !c.is_alphanumeric() {
                return Err(invalid(format!("contains forbidden character {c:?}")));
            }
        }
        // `unwrap`: the empty case is handled above, so there is a first char.
        if !self.0.chars().next().unwrap().is_lowercase() {
            return Err(invalid("must be lowerCamelCase".to_owned()));
        }
        Ok(())
    }

    /// The `UpperCamelCase` form, for use in e.g. method names (`userEvent` ->
    /// `UserEvent`). Ports aip-go's `UpperCamelCase`.
    pub fn upper_camel_case(&self) -> String {
        initial_upper_case(&self.0)
    }
}

impl std::fmt::Display for GrammaticalName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_pass() {
        for name in ["shipper", "shippers", "userEvent", "userEvents", "site2"] {
            GrammaticalName::new(name)
                .validate()
                .unwrap_or_else(|e| panic!("{name:?} should be valid: {e}"));
        }
    }

    #[test]
    fn rejects_empty() {
        let err = GrammaticalName::new("").validate().expect_err("empty");
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn rejects_forbidden_characters() {
        for name in ["user_event", "user-event", "user.event", "user event"] {
            let err = GrammaticalName::new(name)
                .validate()
                .expect_err("not URL-safe");
            assert!(
                err.to_string().contains("forbidden character"),
                "{name:?}: {err}",
            );
        }
    }

    #[test]
    fn rejects_upper_camel_case() {
        let err = GrammaticalName::new("UserEvent")
            .validate()
            .expect_err("not lowerCamelCase");
        assert!(err.to_string().contains("lowerCamelCase"));
    }

    #[test]
    fn upper_camel_case_capitalises_the_first_letter() {
        assert_eq!(
            GrammaticalName::new("userEvent").upper_camel_case(),
            "UserEvent"
        );
        assert_eq!(
            GrammaticalName::new("shipper").upper_camel_case(),
            "Shipper"
        );
    }
}
