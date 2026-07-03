//! Team membership roles. Ported from Coolify `app/Enums/Role.php`
//! (member/admin/owner with a rank ordering and `lt`/`gt` comparisons).

/// A user's role within a team. Serialized snake_case to match the `role`
/// column of the `team_user` pivot and `team_invitations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    Admin,
    Owner,
}

impl Role {
    /// Numeric precedence: member=1, admin=2, owner=3 (parity with Role::rank).
    pub fn rank(self) -> u8 {
        match self {
            Role::Member => 1,
            Role::Admin => 2,
            Role::Owner => 3,
        }
    }

    /// True when `self` ranks strictly below `other` (Role::lt).
    pub fn lt(self, other: Role) -> bool {
        self.rank() < other.rank()
    }

    /// True when `self` ranks strictly above `other` (Role::gt).
    pub fn gt(self, other: Role) -> bool {
        self.rank() > other.rank()
    }

    /// Admin-or-owner: the write/manage tier (User::isAdmin).
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin | Role::Owner)
    }

    /// Owner only (User::isOwner).
    pub fn is_owner(self) -> bool {
        matches!(self, Role::Owner)
    }

    /// The role's canonical snake_case string.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    /// Coerce a stored/user-supplied string to a role, defaulting unknown
    /// values to `Member` (the least-privileged role) rather than failing.
    pub fn from_str_coerce(s: &str) -> Role {
        match s {
            "owner" => Role::Owner,
            "admin" => Role::Admin,
            _ => Role::Member,
        }
    }
}

impl std::str::FromStr for Role {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Role::from_str_coerce(s))
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_matches_contract() {
        assert_eq!(Role::Member.rank(), 1);
        assert_eq!(Role::Admin.rank(), 2);
        assert_eq!(Role::Owner.rank(), 3);
    }

    #[test]
    fn lt_and_gt_by_rank() {
        assert!(Role::Member.lt(Role::Admin));
        assert!(Role::Member.lt(Role::Owner));
        assert!(Role::Admin.lt(Role::Owner));
        assert!(!Role::Admin.lt(Role::Admin));
        assert!(!Role::Owner.lt(Role::Member));

        assert!(Role::Owner.gt(Role::Admin));
        assert!(Role::Owner.gt(Role::Member));
        assert!(Role::Admin.gt(Role::Member));
        assert!(!Role::Admin.gt(Role::Admin));
        assert!(!Role::Member.gt(Role::Owner));
    }

    #[test]
    fn admin_and_owner_predicates() {
        assert!(!Role::Member.is_admin());
        assert!(Role::Admin.is_admin());
        assert!(Role::Owner.is_admin());

        assert!(!Role::Member.is_owner());
        assert!(!Role::Admin.is_owner());
        assert!(Role::Owner.is_owner());
    }

    #[test]
    fn from_str_coerces_unknown_to_member() {
        assert_eq!(Role::from_str_coerce("owner"), Role::Owner);
        assert_eq!(Role::from_str_coerce("admin"), Role::Admin);
        assert_eq!(Role::from_str_coerce("member"), Role::Member);
        assert_eq!(Role::from_str_coerce("bogus"), Role::Member);
        assert_eq!(Role::from_str_coerce(""), Role::Member);
    }

    #[test]
    fn serde_is_snake_case() {
        assert_eq!(serde_json::to_string(&Role::Owner).unwrap(), "\"owner\"");
        let r: Role = serde_json::from_str("\"admin\"").unwrap();
        assert_eq!(r, Role::Admin);
    }
}
