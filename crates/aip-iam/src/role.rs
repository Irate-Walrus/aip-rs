//! IAM **Roles** — the named bundle of **Permissions** a **Binding** grants.

use std::fmt;
use std::str::FromStr;

use crate::Error;

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
}
