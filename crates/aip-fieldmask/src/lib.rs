//! AIP-161 field masks: apply update masks and validate paths.
//!
//! The mask type is `google.protobuf.FieldMask` (`prost_types::FieldMask`) —
//! exactly how it arrives in an update request. Only the *message* side is
//! reflective ([`DynamicMessage`]); the mask itself is plain data.
//!
//! See <https://google.aip.dev/161> and <https://google.aip.dev/134>.

use prost_reflect::{DynamicMessage, MessageDescriptor};
use prost_types::FieldMask;

/// Errors produced when applying or validating a field mask.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("field mask path {path:?} does not exist on message {message}")]
    UnknownPath { path: String, message: String },
    #[error("dst and src message types differ: {dst} vs {src}")]
    TypeMismatch { dst: String, src: String },
}

/// Reports whether `mask` is the full-replacement mask (`["*"]`).
pub fn is_full_replacement(mask: &FieldMask) -> bool {
    mask.paths.len() == 1 && mask.paths[0] == "*"
}

/// Applies an AIP-134 update mask, copying masked fields from `src` into `dst`.
///
/// Empty mask copies `src`'s populated fields; `["*"]` is a full replacement;
/// otherwise each path is copied, and a masked path absent from `src` is
/// cleared in `dst`. Returns [`Error::TypeMismatch`] if the descriptors differ
/// (where `aip-go` panics).
pub fn update(
    _mask: &FieldMask,
    _dst: &mut DynamicMessage,
    _src: &DynamicMessage,
) -> Result<(), Error> {
    todo!("port aip-go fieldmask.Update onto DynamicMessage")
}

/// Validates that every path in `mask` resolves to a field on `descriptor`.
///
/// Takes a descriptor, not a message instance — you need no value to check paths.
pub fn validate(_mask: &FieldMask, _descriptor: &MessageDescriptor) -> Result<(), Error> {
    todo!("resolve each dotted path against the descriptor")
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
    }
}
