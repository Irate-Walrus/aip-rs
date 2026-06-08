//! [`ResourceType`] — an AIP resource type name like
//! `pubsub.googleapis.com/Topic`, split into a service name and a type. Ports
//! aip-go's `aipreflect.ResourceType`.

use crate::Error;

/// An AIP resource type name, e.g. `pubsub.googleapis.com/Topic`.
///
/// The string is a service name (a domain) and a type joined by a single `/`.
/// Construct one with [`ResourceType::new`] and check it with
/// [`ResourceType::validate`]; the accessors operate on whatever string was
/// supplied even if it is malformed (matching aip-go).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceType(String);

impl ResourceType {
    /// Wraps a resource type string. Does not validate — call
    /// [`validate`](Self::validate) for that.
    pub fn new(resource_type: impl Into<String>) -> Self {
        Self(resource_type.into())
    }

    /// The full resource type string, e.g. `pubsub.googleapis.com/Topic`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The service name — everything before the last `/` (e.g.
    /// `pubsub.googleapis.com`), or `""` if there is no `/`.
    pub fn service_name(&self) -> &str {
        match self.0.rfind('/') {
            Some(i) => &self.0[..i],
            None => "",
        }
    }

    /// The type — everything after the last `/` (e.g. `Topic`), or `""` if
    /// there is no `/`.
    pub fn type_name(&self) -> &str {
        match self.0.rfind('/') {
            Some(i) => &self.0[i + 1..],
            None => "",
        }
    }

    /// Checks that the resource type name is syntactically valid: exactly one
    /// `/`, a domain-shaped service name, and an `UpperCamelCase` type.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |reason: &str| Error::InvalidResourceType {
            resource_type: self.0.clone(),
            reason: reason.to_owned(),
        };
        if self.0.matches('/').count() != 1 {
            return Err(invalid("invalid format"));
        }
        validate_service_name(self.service_name()).map_err(invalid)?;
        validate_type(self.type_name()).map_err(invalid)?;
        Ok(())
    }
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A service name must be non-empty and domain-shaped (contain a `.`).
fn validate_service_name(service_name: &str) -> Result<(), &'static str> {
    if service_name.is_empty() {
        return Err("service name: empty");
    }
    if !service_name.contains('.') {
        return Err("service name: must be a valid domain name");
    }
    Ok(())
}

/// A type must be non-empty, start with an upper-case letter, and be made up of
/// letters and digits only (`UpperCamelCase`).
fn validate_type(type_name: &str) -> Result<(), &'static str> {
    let mut chars = type_name.chars();
    match chars.next() {
        None => return Err("type: is empty"),
        Some(first) if !first.is_uppercase() => {
            return Err("type: must start with an upper-case letter")
        }
        Some(_) => {}
    }
    if !type_name.chars().all(char::is_alphanumeric) {
        return Err("type: must be UpperCamelCase");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_service_name_and_type() {
        let rt = ResourceType::new("pubsub.googleapis.com/Topic");
        assert_eq!(rt.service_name(), "pubsub.googleapis.com");
        assert_eq!(rt.type_name(), "Topic");
        assert_eq!(rt.as_str(), "pubsub.googleapis.com/Topic");
    }

    #[test]
    fn valid_type_passes() {
        assert!(ResourceType::new("pubsub.googleapis.com/Topic")
            .validate()
            .is_ok());
        assert!(ResourceType::new("freight-example.einride.tech/Shipper")
            .validate()
            .is_ok());
    }

    #[test]
    fn rejects_malformed_types() {
        for (input, want) in [
            ("pubsub/Topic", "service name: must be a valid domain name"),
            (
                "pubsub.googleapis.com/topic",
                "type: must start with an upper-case letter",
            ),
            (
                "pubsub.googleapis.com/Topic_2",
                "type: must be UpperCamelCase",
            ),
            ("no-slash", "invalid format"),
            ("a/b/c", "invalid format"),
        ] {
            let err = ResourceType::new(input)
                .validate()
                .expect_err("should reject");
            let msg = err.to_string();
            assert!(
                msg.contains(want),
                "input {input:?}: error {msg:?} should contain {want:?}",
            );
        }
    }
}
