//! AIP-122 resource IDs: validate user-settable IDs and generate system IDs.
//!
//! Pure string work plus UUID generation; no protobuf dependency.
//!
//! # Example
//!
//! ```
//! use aip_resourceid::{generate_system, validate_user_settable};
//!
//! // user-settable id: RFC-1034 shape (or UUID)
//! validate_user_settable("acme-01").unwrap();
//! assert!(validate_user_settable("-bad-").is_err());
//!
//! // system-assigned id: UUIDv4, AIP-148
//! let id = generate_system();
//! validate_user_settable(&id).unwrap();
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]
//!
//! A user-settable [`Resource ID`](validate_user_settable) conforms to
//! [RFC-1034]: lower-case letters, numbers, and hyphens, beginning with a
//! letter, ending with a letter or number, and at most 63 characters. UUID-
//! shaped IDs are accepted as-is — AIP-122 used to discourage them, but that
//! guidance was removed ("we no longer saw value in this requirement"), and the
//! reference implementation (`einride/aip-go`) accepts them.
//!
//! See <https://google.aip.dev/122> and <https://google.aip.dev/148>.
//!
//! [RFC-1034]: https://tools.ietf.org/html/rfc1034

/// Errors produced when validating a user-settable resource ID.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The ID is empty or longer than 63 characters.
    #[error("user-settable id must be between 1 and 63 characters (got {len})")]
    Length { len: usize },
    /// The ID does not begin with a letter (and is not UUID-shaped).
    #[error("user-settable id must begin with a letter")]
    LeadingCharacter,
    /// The ID does not end with a letter or a number.
    #[error("user-settable id must end with a letter or number")]
    TrailingCharacter,
    /// The ID contains a character outside the permitted `[a-z0-9-]` set.
    #[error(
        "user-settable id must only contain lowercase letters, numbers, and \
         hyphens (got {character:?} at position {position})"
    )]
    Character {
        /// The offending character.
        character: char,
        /// Its byte offset within the ID.
        position: usize,
    },
}

/// Validates a user-settable resource ID against AIP-122.
///
/// A valid ID is either UUID-shaped (accepted as-is, per AIP-148's companion
/// `uid` and the removed UUID restriction in AIP-122) or conforms to
/// [RFC-1034]: 1–63 characters of lower-case letters, numbers, and hyphens,
/// beginning with a letter and ending with a letter or number.
///
/// Ported from `einride/aip-go`'s `ValidateUserSettable`.
///
/// [RFC-1034]: https://tools.ietf.org/html/rfc1034
pub fn validate_user_settable(id: &str) -> Result<(), Error> {
    // Byte length, matching aip-go's `len(id)`; ASCII ids make this the same as
    // the character count, and non-ASCII is rejected by the character scan.
    let len = id.len();
    if !(1..=63).contains(&len) {
        return Err(Error::Length { len });
    }
    // UUID-shaped ids skip the RFC-1034 shape rules (a UUID begins with a hex
    // digit, not a letter). Lenient parse to match aip-go's `uuid.Parse`.
    if uuid::Uuid::parse_str(id).is_ok() {
        return Ok(());
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_alphabetic() {
        return Err(Error::LeadingCharacter);
    }
    if bytes[len - 1] == b'-' {
        return Err(Error::TrailingCharacter);
    }
    for (position, character) in id.char_indices() {
        match character {
            'a'..='z' | '0'..='9' | '-' => {}
            _ => {
                return Err(Error::Character {
                    character,
                    position,
                })
            }
        }
    }
    Ok(())
}

/// Generates a system-assigned resource ID (a UUIDv4 string), per AIP-148.
pub fn generate_system() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `reason` + `domain` (`aip-rs`) and the
    /// error's dynamic values as `metadata`. A resource ID is an opaque value
    /// rather than a request field path, so no `BadRequest` is attached.
    /// See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::Length { len } => (
                "RESOURCE_ID_LENGTH",
                HashMap::from([("length".to_owned(), len.to_string())]),
            ),
            Error::LeadingCharacter => ("RESOURCE_ID_LEADING_CHARACTER", HashMap::new()),
            Error::TrailingCharacter => ("RESOURCE_ID_TRAILING_CHARACTER", HashMap::new()),
            Error::Character {
                character,
                position,
            } => (
                "RESOURCE_ID_INVALID_CHARACTER",
                HashMap::from([
                    ("character".to_owned(), character.to_string()),
                    ("position".to_owned(), position.to_string()),
                ]),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn invalid_character_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::Character {
            character: '!',
            position: 3,
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "RESOURCE_ID_INVALID_CHARACTER");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.metadata.get("position").map(String::as_str), Some("3"));

        // A resource ID is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected outcome of [`validate_user_settable`], collapsing each error
    /// variant to a discriminant so the table can assert behaviour without
    /// matching on private message text.
    #[derive(Debug, PartialEq, Eq)]
    enum Expect {
        Ok,
        Length,
        Leading,
        Trailing,
        Character,
    }

    fn classify(result: Result<(), Error>) -> Expect {
        match result {
            Ok(()) => Expect::Ok,
            Err(Error::Length { .. }) => Expect::Length,
            Err(Error::LeadingCharacter) => Expect::Leading,
            Err(Error::TrailingCharacter) => Expect::Trailing,
            Err(Error::Character { .. }) => Expect::Character,
        }
    }

    #[test]
    fn validate_user_settable_table() {
        // Ported from aip-go resourceid `TestValidateUserSettable`. The UUID rows
        // are accepted, matching aip-go and current AIP-122 (the UUID restriction
        // was removed).
        let long = "a".repeat(64);
        let cases: &[(&str, Expect)] = &[
            ("abcd", Expect::Ok),
            ("abcd-efgh-1234", Expect::Ok),
            ("", Expect::Length),
            (long.as_str(), Expect::Length),
            ("-abc", Expect::Leading),
            ("abc-", Expect::Trailing),
            ("123-abc", Expect::Leading),
            ("daf1cb3e-f33b-43f1-81cc-e65fda51efa5", Expect::Ok),
            ("49351204-7395-47f1-9681-d48044b48c71", Expect::Ok),
            ("abcd/efgh", Expect::Character),
        ];
        for (id, expected) in cases {
            assert_eq!(
                classify(validate_user_settable(id)),
                *expected,
                "validate_user_settable({id:?})"
            );
        }
    }

    #[test]
    fn single_letter_is_valid() {
        assert!(validate_user_settable("a").is_ok());
    }

    #[test]
    fn generate_system_is_uuid_v4() {
        // Ported from aip-go resourceid `TestNewSystemGenerated`.
        let id = generate_system();
        let parsed = uuid::Uuid::parse_str(&id).expect("generate_system returns a parseable uuid");
        assert_eq!(
            parsed.get_version_num(),
            4,
            "system id must be a UUIDv4: {id}"
        );
        // Canonical lower-case hyphenated form (what aip-go's regex asserts).
        assert_eq!(id, parsed.hyphenated().to_string());
        assert_eq!(id.len(), 36);
        // A generated system id is itself an acceptable resource id.
        assert!(validate_user_settable(&id).is_ok());
    }
}
