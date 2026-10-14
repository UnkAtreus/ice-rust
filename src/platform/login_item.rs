//! Launch at login through `SMAppService.mainApp` (macOS 13+).

use objc2_service_management::SMAppService;

use crate::login_item::{LoginState, state_from_status};
use crate::update::{bundle_of_executable, is_translocated};

/// Only an app bundle can register itself as a login item.
fn bundled() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| bundle_of_executable(&exe))
        .is_some_and(|bundle| !is_translocated(&bundle))
}

/// The login item's current state.
pub fn state() -> LoginState {
    if !bundled() {
        return LoginState::Unavailable;
    }
    // SAFETY: plain class and instance calls without arguments; the service
    // object is owned by ServiceManagement.
    let status = unsafe { SMAppService::mainAppService().status() };
    state_from_status(status.0)
}

/// Registers or unregisters the app as a login item.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if !bundled() {
        return Err("Launch at login needs ice-rust to run as IceRust.app".into());
    }
    // SAFETY: as in `state`; both calls report failure through NSError.
    let result = unsafe {
        let service = SMAppService::mainAppService();
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };
    result.map_err(|error| error.localizedDescription().to_string())
}

/// Opens System Settings > General > Login Items.
pub fn open_login_items() {
    // SAFETY: a class method without arguments.
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}
