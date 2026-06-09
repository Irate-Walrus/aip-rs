//! String-case helpers ported from aip-go's `reflect/aipreflect/strcase.go`.
//!
//! These exist only to drive code generation (e.g. turning a grammatical name
//! into a method-name prefix), which is why they live here rather than in the
//! runtime `aip-reflect` crate (ADR-0011).

/// Upper-cases the first character of `s`, leaving the rest untouched.
///
/// Ports aip-go's `initialUpperCase`: the first [`char`] is mapped to upper
/// case (which may expand to more than one char, e.g. `ß`), and the remainder
/// is copied verbatim. An empty string is returned unchanged.
pub fn initial_upper_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_cases_only_the_first_character() {
        assert_eq!(initial_upper_case("shipper"), "Shipper");
        assert_eq!(initial_upper_case("userEvents"), "UserEvents");
    }

    #[test]
    fn leaves_an_already_upper_first_character() {
        assert_eq!(initial_upper_case("Shipper"), "Shipper");
    }

    #[test]
    fn empty_string_is_unchanged() {
        assert_eq!(initial_upper_case(""), "");
    }

    #[test]
    fn single_character() {
        assert_eq!(initial_upper_case("a"), "A");
    }
}
