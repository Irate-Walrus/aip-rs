//! IAM **Roles** — the named bundle of **Permissions** a **Binding** grants.

use std::fmt;
use std::str::FromStr;

use crate::{Error, Permission};

/// A `google.iam.v1` role name in one of its three forms, rendered back losslessly
/// by [`Display`](fmt::Display).
///
/// The custom-role forms keep the parent (`organizations/{o}` or `projects/{p}`)
/// and the trailing role id apart so a caller can scope a role without re-splitting
/// the string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    /// `roles/{role}` — a curated, predefined role (e.g. `roles/viewer`).
    Predefined(String),
    /// `organizations/{org}/roles/{role}` — an organization-scoped custom role.
    Organization {
        /// The organization id.
        organization: String,
        /// The custom role id.
        role: String,
    },
    /// `projects/{project}/roles/{role}` — a project-scoped custom role.
    Project {
        /// The project id.
        project: String,
        /// The custom role id.
        role: String,
    },
}

impl FromStr for Role {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let malformed = || Error::RoleMalformed { role: s.to_owned() };
        let nonempty = |seg: &str| (!seg.is_empty()).then(|| seg.to_owned());
        let segments: Vec<&str> = s.split('/').collect();
        match segments.as_slice() {
            ["roles", role] => nonempty(role).map(Role::Predefined).ok_or_else(malformed),
            ["organizations", org, "roles", role] => match (nonempty(org), nonempty(role)) {
                (Some(organization), Some(role)) => Ok(Role::Organization { organization, role }),
                _ => Err(malformed()),
            },
            ["projects", project, "roles", role] => match (nonempty(project), nonempty(role)) {
                (Some(project), Some(role)) => Ok(Role::Project { project, role }),
                _ => Err(malformed()),
            },
            _ => Err(malformed()),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Predefined(role) => write!(f, "roles/{role}"),
            Role::Organization { organization, role } => {
                write!(f, "organizations/{organization}/roles/{role}")
            }
            Role::Project { project, role } => write!(f, "projects/{project}/roles/{role}"),
        }
    }
}

/// A const **Role**→**Permission** catalogue: the *mechanism* for role→permission
/// expansion, shipping **no role definitions** of its own.
///
/// `aip-iam` deliberately defines no roles — expansion is the consumer's job
/// (ADR-0010). But every consumer then invents the same shape: a match on the role
/// string returning permission tiers, with the supersets (`viewer ⊂ editor ⊂
/// admin`) composed by concatenation so a literal is not repeated per tier. This
/// type makes that contract concrete without taking a position on *which* roles
/// exist: the caller supplies the table, this owns the lookup-and-flatten.
///
/// Const-constructible end to end: it builds on [`Permission::from_static`], so a
/// catalogue is a `const` whose permission literals are validated at compile time.
/// Each role maps to an ordered list of **tiers** (each tier a `&[Permission]`),
/// expanded by concatenation — so `editor` lists `&[VIEWER, EDITOR_EXTRA]` and
/// names the viewer permissions once, by reference, rather than re-spelling them.
/// An **unrecognised role expands to nothing** (an empty iterator), never an error.
///
/// ```
/// use aip_iam::{Permission, RoleSet};
///
/// const VIEWER: &[Permission] = &[Permission::from_static("freight.shippers.get")];
/// const EDITOR_EXTRA: &[Permission] =
///     &[Permission::from_static("freight.shippers.create")];
///
/// const CATALOGUE: RoleSet = RoleSet::new(&[
///     ("roles/freight.viewer", &[VIEWER]),
///     ("roles/freight.editor", &[VIEWER, EDITOR_EXTRA]),
/// ]);
///
/// // editor expands to viewer's permissions plus its own extras.
/// let editor: Vec<_> = CATALOGUE.permissions("roles/freight.editor").collect();
/// assert_eq!(editor.len(), 2);
///
/// // An unrecognised role bundles nothing.
/// assert_eq!(CATALOGUE.permissions("roles/freight.ghost").count(), 0);
/// ```
pub struct RoleSet {
    /// `(role name, tiers)` entries; each tier a slice of **Permissions** the role
    /// bundles, concatenated in order on lookup.
    roles: &'static [(&'static str, &'static [&'static [Permission]])],
}

impl RoleSet {
    /// Build a catalogue from a static table of `(role, tiers)` entries.
    ///
    /// `const`, so a catalogue lives in a `const`/`static` and its
    /// [`Permission::from_static`] literals are validated at compile time. The
    /// table is the *caller's* role definitions — `aip-iam` ships none (ADR-0010).
    pub const fn new(
        roles: &'static [(&'static str, &'static [&'static [Permission]])],
    ) -> RoleSet {
        RoleSet { roles }
    }

    /// The **Permissions** `role` bundles: every tier of its entry, concatenated in
    /// order. An unrecognised `role` yields an empty iterator — expansion never
    /// errors, it just bundles nothing (the closest analog to "this role grants no
    /// permissions here").
    ///
    /// Borrows from the catalogue, so the yielded `&Permission`s cost no allocation;
    /// a caller wanting owned values [`clone`](Clone::clone)s or `.collect()`s them.
    pub fn permissions(&self, role: &str) -> impl Iterator<Item = &Permission> {
        self.roles
            .iter()
            .find(|entry| entry.0 == role)
            .into_iter()
            .flat_map(|entry| entry.1.iter().copied().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_form() {
        let forms = [
            "roles/viewer",
            "roles/storage.objectAdmin",
            "organizations/123/roles/customAuditor",
            "projects/my-project/roles/customDeployer",
        ];
        for form in forms {
            let role: Role = form.parse().expect("recognised role parses");
            assert_eq!(role.to_string(), form, "round-trip {form:?}");
        }
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "viewer",
            "roles/",
            "roles",
            "projects/p/roles/",
            "folders/1/roles/x",
        ] {
            assert_eq!(
                bad.parse::<Role>(),
                Err(Error::RoleMalformed {
                    role: bad.to_owned()
                }),
                "{bad:?} should be malformed"
            );
        }
    }

    const VIEWER: &[Permission] = &[
        Permission::from_static("freight.shippers.get"),
        Permission::from_static("freight.shippers.list"),
    ];
    const EDITOR_EXTRA: &[Permission] = &[Permission::from_static("freight.shippers.create")];

    const CATALOGUE: RoleSet = RoleSet::new(&[
        ("roles/freight.viewer", &[VIEWER]),
        ("roles/freight.editor", &[VIEWER, EDITOR_EXTRA]),
    ]);

    #[test]
    fn tiers_compose_by_concatenation_without_repeating_literals() {
        // viewer is its single tier; editor is viewer's tier plus its own extra,
        // so editor's expansion is a superset naming the viewer literals once.
        let viewer: Vec<&Permission> = CATALOGUE.permissions("roles/freight.viewer").collect();
        assert_eq!(
            viewer,
            vec![
                &Permission::from_static("freight.shippers.get"),
                &Permission::from_static("freight.shippers.list"),
            ],
        );

        let editor: Vec<&Permission> = CATALOGUE.permissions("roles/freight.editor").collect();
        assert_eq!(
            editor,
            vec![
                &Permission::from_static("freight.shippers.get"),
                &Permission::from_static("freight.shippers.list"),
                &Permission::from_static("freight.shippers.create"),
            ],
            "editor = viewer tier ++ editor extra",
        );
    }

    #[test]
    fn an_unrecognised_role_expands_to_nothing() {
        assert_eq!(CATALOGUE.permissions("roles/freight.ghost").count(), 0);
        // An empty catalogue knows no roles at all.
        const EMPTY: RoleSet = RoleSet::new(&[]);
        assert_eq!(EMPTY.permissions("roles/freight.viewer").count(), 0);
    }
}
