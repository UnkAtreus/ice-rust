//! Global hotkeys through Carbon (v1.9), as Ice does
//! (`HotkeyRegistry.swift`): `RegisterEventHotKey` on the event dispatcher
//! target and one handler for hotkey presses, installed with the first
//! hotkey. Carbon hotkeys need no permission. Also the system shortcuts a
//! hotkey must not take, and key names in the current keyboard layout.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_core_foundation::{CFArray, CFBoolean, CFData, CFNumber, CFRetained, CFString, CFType};

use super::timer;
use crate::hotkeys::{HotkeyAction, Hotkeys, KeyCombo, special_key_name};

type OsStatus = i32;
type EventHandler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> OsStatus;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

// Carbon (HIToolbox: CarbonEvents.h, TextInputSources.h,
// CarbonEventsCore.h) and CoreServices (UnicodeUtilities.h):
//   EventTargetRef GetEventDispatcherTarget(void);
//   OSStatus InstallEventHandler(EventTargetRef, EventHandlerUPP,
//       ItemCount, const EventTypeSpec *, void *, EventHandlerRef *);
//   OSStatus RegisterEventHotKey(UInt32 keyCode, UInt32 modifiers,
//       EventHotKeyID, EventTargetRef, OptionBits, EventHotKeyRef *);
//   OSStatus UnregisterEventHotKey(EventHotKeyRef);
//   OSStatus GetEventParameter(EventRef, EventParamName, EventParamType,
//       EventParamType *, ByteCount, ByteCount *, void *);
//   OSStatus CopySymbolicHotKeys(CFArrayRef *);
//   TISInputSourceRef TISCopyCurrentKeyboardLayoutInputSource(void);
//   void *TISGetInputSourceProperty(TISInputSourceRef, CFStringRef);
//   UInt8 LMGetKbdType(void);
//   OSStatus UCKeyTranslate(const UCKeyboardLayout *, UInt16, UInt16,
//       UInt32, UInt32, OptionBits, UInt32 *, UniCharCount,
//       UniCharCount *, UniChar *);
// ItemCount, ByteCount and UniCharCount are unsigned long; OSType,
// OptionBits and the event param names and types are UInt32; the Ref
// types are pointers.
// SAFETY: the declarations below match those headers.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetEventDispatcherTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandler,
        count: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> OsStatus;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OsStatus;
    fn CopySymbolicHotKeys(out: *mut *const CFArray) -> OsStatus;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut CFType;
    fn TISGetInputSourceProperty(source: &CFType, key: &CFString) -> *const CFType;
    fn LMGetKbdType() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: Option<&'static CFString>;
}

// The keys of a symbolic hotkey's dictionary are CFSTR macros in
// CarbonEvents.h, not exported symbols.
const SYMBOLIC_CODE: &str = "kHISymbolicHotKeyCode";
const SYMBOLIC_MODIFIERS: &str = "kHISymbolicHotKeyModifiers";
const SYMBOLIC_ENABLED: &str = "kHISymbolicHotKeyEnabled";

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn UCKeyTranslate(
        layout: *const c_void,
        key_code: u16,
        key_action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        chars: *mut u16,
    ) -> OsStatus;
}

/// `'IcRs'`, the signature of our hotkey ids.
const SIGNATURE: u32 = u32::from_be_bytes(*b"IcRs");
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const NO_ERR: OsStatus = 0;
const EVENT_NOT_HANDLED: OsStatus = -9874;
const KEY_ACTION_DISPLAY: u16 = 3;
const NO_DEAD_KEYS: u32 = 1;
/// Command, Shift, Option and Control in Carbon's modifier flags; a
/// symbolic hotkey can carry more bits, such as caps lock.
const CARBON_MODIFIER_MASK: u32 = 0x1B00;

/// Why hotkeys are off for a while.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suspend {
    /// One of our menus is tracking (Ice does the same).
    Menu,
    /// The Hotkeys page is recording a shortcut, which must reach the
    /// recorder instead of running an action.
    Recording,
}

impl Suspend {
    fn bit(self) -> u8 {
        match self {
            Suspend::Menu => 1,
            Suspend::Recording => 2,
        }
    }
}

#[derive(Default)]
struct Center {
    handler_installed: bool,
    registered: Vec<*mut c_void>,
    wanted: Hotkeys,
    /// `Suspend` bits.
    suspended: u8,
    on_press: Option<Rc<dyn Fn(HotkeyAction)>>,
}

thread_local! {
    static CENTER: RefCell<Center> = RefCell::new(Center::default());
}

/// Called on the main thread for every press of one of our hotkeys.
pub fn set_handler(on_press: impl Fn(HotkeyAction) + 'static) {
    CENTER.with(|c| c.borrow_mut().on_press = Some(Rc::new(on_press)));
}

extern "C" fn on_hot_key(_call: *mut c_void, event: *mut c_void, _user: *mut c_void) -> OsStatus {
    let mut id = EventHotKeyId::default();
    // SAFETY: `event` is the hotkey event Carbon passed, and `id` is a
    // buffer of the size given for the requested 'hkid' parameter.
    let err = unsafe {
        GetEventParameter(
            event,
            PARAM_DIRECT_OBJECT,
            TYPE_HOT_KEY_ID,
            ptr::null_mut(),
            size_of::<EventHotKeyId>(),
            ptr::null_mut(),
            (&raw mut id).cast(),
        )
    };
    if err != NO_ERR || id.signature != SIGNATURE {
        return EVENT_NOT_HANDLED;
    }
    let Some(action) = HotkeyAction::from_id(id.id) else {
        return EVENT_NOT_HANDLED;
    };
    // Carbon calls this on the main thread. The action runs on the next
    // pass of the run loop, so it may register hotkeys again.
    if let Some(mtm) = MainThreadMarker::new() {
        timer::after(mtm, std::time::Duration::ZERO, move || {
            let callback = CENTER.with(|c| c.borrow().on_press.clone());
            if let Some(callback) = callback {
                callback(action);
            }
        });
    }
    NO_ERR
}

fn install_handler(center: &mut Center) -> bool {
    if center.handler_installed {
        return true;
    }
    let spec = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOT_KEY_PRESSED,
    };
    // SAFETY: one valid spec; the handler is a plain function for the
    // whole run, and the handler reference is not needed later.
    let err = unsafe {
        InstallEventHandler(
            GetEventDispatcherTarget(),
            on_hot_key,
            1,
            &spec,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    center.handler_installed = err == NO_ERR;
    if err != NO_ERR {
        eprintln!("ice-rust: could not install the hotkey handler ({err})");
    }
    center.handler_installed
}

fn unregister_all(center: &mut Center) {
    for hot_key in center.registered.drain(..) {
        // SAFETY: each reference came from RegisterEventHotKey and is
        // unregistered once.
        unsafe { UnregisterEventHotKey(hot_key) };
    }
}

/// Registers what `center` wants unless suspended. Returns the actions
/// macOS refused, for example a combination another app registered.
fn apply(center: &mut Center) -> Vec<HotkeyAction> {
    unregister_all(center);
    if center.suspended != 0 {
        return Vec::new();
    }
    let wanted: Vec<(HotkeyAction, KeyCombo)> = center.wanted.assigned().collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    if !install_handler(center) {
        return wanted.into_iter().map(|(action, _)| action).collect();
    }
    let mut refused = Vec::new();
    for (action, combo) in wanted {
        let mut hot_key = ptr::null_mut();
        // SAFETY: plain values and a valid out pointer.
        let err = unsafe {
            RegisterEventHotKey(
                u32::from(combo.key_code),
                combo.carbon_modifiers(),
                EventHotKeyId {
                    signature: SIGNATURE,
                    id: action.id(),
                },
                GetEventDispatcherTarget(),
                0,
                &mut hot_key,
            )
        };
        if err == NO_ERR && !hot_key.is_null() {
            center.registered.push(hot_key);
        } else {
            eprintln!(
                "ice-rust: macOS refused the hotkey for {} ({err})",
                action.key()
            );
            refused.push(action);
        }
    }
    refused
}

/// Registers `hotkeys` in place of the earlier ones. Returns the actions
/// macOS refused.
pub fn register(hotkeys: &Hotkeys) -> Vec<HotkeyAction> {
    CENTER.with(|c| {
        let mut center = c.borrow_mut();
        center.wanted = *hotkeys;
        apply(&mut center)
    })
}

/// Unregisters every hotkey while `reason` holds, and registers them again
/// when no reason is left.
pub fn set_suspended(reason: Suspend, on: bool) {
    CENTER.with(|c| {
        let mut center = c.borrow_mut();
        let before = center.suspended;
        if on {
            center.suspended |= reason.bit();
        } else {
            center.suspended &= !reason.bit();
        }
        if (before == 0) != (center.suspended == 0) {
            apply(&mut center);
        }
    });
}

/// Whether `combo` is an enabled system shortcut (System Settings >
/// Keyboard > Keyboard Shortcuts), which a hotkey must not take.
pub fn is_system_shortcut(combo: &KeyCombo) -> bool {
    let mut array: *const CFArray = ptr::null();
    // SAFETY: a valid out pointer; the Copy rule makes us the owner.
    let err = unsafe { CopySymbolicHotKeys(&mut array) };
    let Some(array) = NonNull::new(array.cast_mut()).filter(|_| err == NO_ERR) else {
        return false;
    };
    // SAFETY: the array came from a Copy function.
    let array = unsafe { CFRetained::from_raw(array) };
    let code_key = CFString::from_static_str(SYMBOLIC_CODE);
    let modifiers_key = CFString::from_static_str(SYMBOLIC_MODIFIERS);
    let enabled_key = CFString::from_static_str(SYMBOLIC_ENABLED);
    // SAFETY: the array holds CFDictionaries, which are CFTypes.
    let entries = unsafe { array.cast_unchecked::<CFType>() };
    (0..entries.len())
        .filter_map(|i| entries.get(i))
        .any(|entry| {
            let field = |key: &CFString| -> Option<CFRetained<CFType>> {
                let dict = entry.downcast_ref::<objc2_core_foundation::CFDictionary>()?;
                // SAFETY: CFDictionaryGetValue with a CFString key; the
                // value is a CFType owned by the dictionary, retained here.
                let value = unsafe { dict.value((key as *const CFString).cast()) };
                let value = NonNull::new(value.cast_mut().cast::<CFType>())?;
                // SAFETY: a valid CFType pointer from the dictionary.
                Some(unsafe { CFRetained::retain(value) })
            };
            let number = |key: &CFString| {
                field(key)
                    .and_then(|v| v.downcast::<CFNumber>().ok())
                    .and_then(|n| n.as_i64())
            };
            let enabled = field(&enabled_key)
                .and_then(|v| v.downcast::<CFBoolean>().ok())
                .is_some_and(|b| b.as_bool());
            enabled
                && number(&code_key) == Some(i64::from(combo.key_code))
                && number(&modifiers_key)
                    .is_some_and(|m| m as u32 & CARBON_MODIFIER_MASK == combo.carbon_modifiers())
        })
}

/// The key's name: a fixed name for keys that type no character, else what
/// the key types in the current keyboard layout, upper-cased.
pub fn key_name(key_code: u16) -> String {
    if let Some(name) = special_key_name(key_code) {
        return name.to_string();
    }
    typed_character(key_code)
        .map(|c| c.to_uppercase())
        .unwrap_or_else(|| format!("Key {key_code}"))
}

fn typed_character(key_code: u16) -> Option<String> {
    // SAFETY: the Copy rule makes us the owner; null is handled.
    let source = NonNull::new(unsafe { TISCopyCurrentKeyboardLayoutInputSource() })
        .map(|s| unsafe { CFRetained::from_raw(s) })?;
    // SAFETY: an immutable CFString constant exported by Carbon.
    let key = unsafe { kTISPropertyUnicodeKeyLayoutData }?;
    // SAFETY: `source` is a live input source; the property follows the
    // Get rule and lives as long as the source.
    let data = unsafe { TISGetInputSourceProperty(&source, key) };
    // SAFETY: a non-null property is a CFData of the layout.
    let data = unsafe { data.as_ref() }?.downcast_ref::<CFData>()?;
    let bytes = data.to_vec();
    let mut dead_keys = 0u32;
    let mut chars = [0u16; 4];
    let mut length = 0usize;
    // SAFETY: `bytes` holds the UCKeyboardLayout, and the out buffers have
    // the sizes passed.
    let err = unsafe {
        UCKeyTranslate(
            bytes.as_ptr().cast(),
            key_code,
            KEY_ACTION_DISPLAY,
            0,
            u32::from(LMGetKbdType()),
            NO_DEAD_KEYS,
            &mut dead_keys,
            chars.len(),
            &mut length,
            chars.as_mut_ptr(),
        )
    };
    if err != NO_ERR || length == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&chars[..length.min(chars.len())]);
    (!text.trim().is_empty()).then_some(text)
}
