//! [`MethodType`] — an AIP standard/custom method type. Ports aip-go's
//! `reflect/aipreflect.MethodType` (and its generated `String`/`NamePrefix`/
//! `IsPlural`), a codegen-only helper for reasoning about a service's methods.

/// An AIP method type — the standard methods (AIP-131..135), the soft-delete
/// `Undelete` (AIP-164), the batch methods (AIP-231..235), and the custom
/// `Search` method (AIP-136).
///
/// The discriminants match aip-go's iota order so [`as_str`](Self::as_str) is a
/// faithful port of its generated `stringer` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MethodType {
    /// No method type.
    None,
    /// The AIP-131 standard `Get` method.
    Get,
    /// The AIP-132 standard `List` method.
    List,
    /// The AIP-133 standard `Create` method.
    Create,
    /// The AIP-134 standard `Update` method.
    Update,
    /// The AIP-135 standard `Delete` method.
    Delete,
    /// The AIP-164 `Undelete` method for soft delete.
    Undelete,
    /// The AIP-231 standard `BatchGet` method.
    BatchGet,
    /// The AIP-233 standard `BatchCreate` method.
    BatchCreate,
    /// The AIP-234 standard `BatchUpdate` method.
    BatchUpdate,
    /// The AIP-235 standard `BatchDelete` method.
    BatchDelete,
    /// The AIP-136 custom method for searching a resource collection.
    Search,
}

impl MethodType {
    /// The method name prefix, e.g. `BatchGet` for [`BatchGet`](Self::BatchGet).
    ///
    /// Ports aip-go's `NamePrefix`; for [`None`](Self::None) the prefix is
    /// `"None"`, matching its stringer output.
    pub fn name_prefix(self) -> &'static str {
        self.as_str()
    }

    /// The method type's name, e.g. `"List"` or `"BatchCreate"`.
    ///
    /// Faithful to aip-go's generated `MethodType.String()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Get => "Get",
            Self::List => "List",
            Self::Create => "Create",
            Self::Update => "Update",
            Self::Delete => "Delete",
            Self::Undelete => "Undelete",
            Self::BatchGet => "BatchGet",
            Self::BatchCreate => "BatchCreate",
            Self::BatchUpdate => "BatchUpdate",
            Self::BatchDelete => "BatchDelete",
            Self::Search => "Search",
        }
    }

    /// Whether the method type relates to a plurality of resources (the list,
    /// search, and batch methods). Ports aip-go's `IsPlural`.
    pub fn is_plural(self) -> bool {
        matches!(
            self,
            Self::List
                | Self::Search
                | Self::BatchGet
                | Self::BatchCreate
                | Self::BatchUpdate
                | Self::BatchDelete
        )
    }
}

impl std::fmt::Display for MethodType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_matches_aip_go_stringer() {
        // The concatenated names in aip-go's _MethodType_name, in order.
        assert_eq!(MethodType::None.as_str(), "None");
        assert_eq!(MethodType::Get.as_str(), "Get");
        assert_eq!(MethodType::List.as_str(), "List");
        assert_eq!(MethodType::Create.as_str(), "Create");
        assert_eq!(MethodType::Update.as_str(), "Update");
        assert_eq!(MethodType::Delete.as_str(), "Delete");
        assert_eq!(MethodType::Undelete.as_str(), "Undelete");
        assert_eq!(MethodType::BatchGet.as_str(), "BatchGet");
        assert_eq!(MethodType::BatchCreate.as_str(), "BatchCreate");
        assert_eq!(MethodType::BatchUpdate.as_str(), "BatchUpdate");
        assert_eq!(MethodType::BatchDelete.as_str(), "BatchDelete");
        assert_eq!(MethodType::Search.as_str(), "Search");
    }

    #[test]
    fn name_prefix_equals_the_name() {
        assert_eq!(MethodType::BatchGet.name_prefix(), "BatchGet");
        assert_eq!(MethodType::Get.name_prefix(), "Get");
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(MethodType::BatchCreate.to_string(), "BatchCreate");
    }

    #[test]
    fn plural_methods_are_the_list_search_and_batch_methods() {
        for plural in [
            MethodType::List,
            MethodType::Search,
            MethodType::BatchGet,
            MethodType::BatchCreate,
            MethodType::BatchUpdate,
            MethodType::BatchDelete,
        ] {
            assert!(plural.is_plural(), "{plural} should be plural");
        }
        for singular in [
            MethodType::None,
            MethodType::Get,
            MethodType::Create,
            MethodType::Update,
            MethodType::Delete,
            MethodType::Undelete,
        ] {
            assert!(!singular.is_plural(), "{singular} should be singular");
        }
    }
}
