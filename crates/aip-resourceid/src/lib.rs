//! AIP-122 resource IDs: validate user-settable IDs and generate system IDs.
//!
//! Pure string work plus UUID generation; no protobuf dependency.

/// Errors produced when validating a resource ID.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("resource id is empty")]
    Empty,
    #[error("resource id {0:?} is not a valid user-settable id")]
    Invalid(String),
}

/// Validates a user-settable resource ID against AIP-122 rules
/// (character set, length, not a UUID, etc.).
pub fn validate_user_settable(_id: &str) -> Result<(), Error> {
    todo!("enforce AIP-122 user-settable id constraints")
}

/// Generates a system-generated resource ID (UUIDv4), per AIP-148.
pub fn generate_system() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
    }
}
