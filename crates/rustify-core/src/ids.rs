/// Generate a new external identifier. All Rustify external IDs are CUID2
/// (24-char lowercase alphanumeric), stored in `uuid` columns alongside a
/// `BIGSERIAL id`.
pub fn new_uuid() -> String {
    cuid2::create_id()
}

/// Generate a 32-char CUID2 for team invitations (parity with Coolify's
/// `new Cuid2(32)` in `app/Livewire/Team/InviteLink.php`).
pub fn new_invitation_uuid() -> String {
    cuid2::CuidConstructor::new().with_length(32).create_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_cuid2_shaped() {
        let a = new_uuid();
        let b = new_uuid();
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert_eq!(id.len(), 24, "cuid2 default length");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "cuid2 is lowercase alphanumeric: {id}"
            );
            assert!(
                id.chars().next().is_some_and(|c| c.is_ascii_lowercase()),
                "cuid2 starts with a letter: {id}"
            );
        }
    }
}
