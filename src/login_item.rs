//! Launch at login (v1.3), the pure half: what the General page shows for
//! each `SMAppService` status. `platform::login_item` talks to macOS.

/// The login item as the General page sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginState {
    /// Registered and allowed to open at login.
    Enabled,
    /// Registered, waiting for the user to allow it in System Settings.
    RequiresApproval,
    NotRegistered,
    /// macOS cannot find the app's login item.
    NotFound,
    /// The binary does not run from an app bundle (for example `cargo run`).
    Unavailable,
}

/// `SMAppServiceStatus` raw values (ServiceManagement/SMAppService.h).
pub fn state_from_status(status: isize) -> LoginState {
    match status {
        1 => LoginState::Enabled,
        2 => LoginState::RequiresApproval,
        3 => LoginState::NotFound,
        _ => LoginState::NotRegistered,
    }
}

/// Whether the switch shows on.
pub fn switch_on(state: LoginState) -> bool {
    matches!(state, LoginState::Enabled | LoginState::RequiresApproval)
}

/// Whether the switch can be used.
pub fn switch_enabled(state: LoginState) -> bool {
    state != LoginState::Unavailable
}

/// The note under the switch.
pub fn note(state: LoginState) -> &'static str {
    match state {
        LoginState::RequiresApproval => {
            "Approve IceRust in System Settings > General > Login Items"
        }
        LoginState::Unavailable => "Available when ice-rust runs as IceRust.app",
        LoginState::Enabled | LoginState::NotRegistered | LoginState::NotFound => {
            "Open ice-rust when you log in"
        }
    }
}

/// Whether to show the button that opens Login Items.
pub fn needs_approval_button(state: LoginState) -> bool {
    state == LoginState::RequiresApproval
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_values_map_to_states() {
        assert_eq!(state_from_status(0), LoginState::NotRegistered);
        assert_eq!(state_from_status(1), LoginState::Enabled);
        assert_eq!(state_from_status(2), LoginState::RequiresApproval);
        assert_eq!(state_from_status(3), LoginState::NotFound);
        assert_eq!(state_from_status(42), LoginState::NotRegistered);
    }

    #[test]
    fn switch_follows_registration() {
        assert!(switch_on(LoginState::Enabled));
        assert!(switch_on(LoginState::RequiresApproval));
        assert!(!switch_on(LoginState::NotRegistered));
        assert!(!switch_on(LoginState::NotFound));
        assert!(!switch_on(LoginState::Unavailable));
    }

    #[test]
    fn unbundled_disables_the_switch_with_a_note() {
        assert!(!switch_enabled(LoginState::Unavailable));
        assert!(switch_enabled(LoginState::NotRegistered));
        assert_eq!(
            note(LoginState::Unavailable),
            "Available when ice-rust runs as IceRust.app"
        );
    }

    #[test]
    fn approval_shows_the_note_and_the_button() {
        assert!(needs_approval_button(LoginState::RequiresApproval));
        assert!(!needs_approval_button(LoginState::Enabled));
        assert!(note(LoginState::RequiresApproval).contains("Login Items"));
    }
}
