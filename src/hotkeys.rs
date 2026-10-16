//! Global hotkeys (v1.9), like Ice's: an action per hotkey, none assigned
//! by default, stored under the defaults key `Hotkeys` as JSON
//! `{action: [keyCode, modifiers]}` with Ice's modifier bits (control 1,
//! option 2, shift 4, command 8). `platform::hotkeys` registers them with
//! Carbon, which needs no permission. Pure.

use std::fmt;

use serde_json::{Map, Value};

use crate::triggers::Modifiers;

/// What a hotkey does. The names are Ice's (`HotkeyAction.swift`) and the
/// stored keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    ToggleHiddenSection,
    ToggleAlwaysHiddenSection,
    /// Turns Use Ice Bar on or off.
    EnableIceBar,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 3] = [
        HotkeyAction::ToggleHiddenSection,
        HotkeyAction::ToggleAlwaysHiddenSection,
        HotkeyAction::EnableIceBar,
    ];

    pub fn key(self) -> &'static str {
        match self {
            HotkeyAction::ToggleHiddenSection => "ToggleHiddenSection",
            HotkeyAction::ToggleAlwaysHiddenSection => "ToggleAlwaysHiddenSection",
            HotkeyAction::EnableIceBar => "EnableIceBar",
        }
    }

    pub fn from_key(key: &str) -> Option<HotkeyAction> {
        Self::ALL.into_iter().find(|a| a.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            HotkeyAction::ToggleHiddenSection => "Toggle the hidden section",
            HotkeyAction::ToggleAlwaysHiddenSection => "Toggle the always-hidden section",
            HotkeyAction::EnableIceBar => "Turn the Ice Bar on or off",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            HotkeyAction::ToggleHiddenSection => "Like a click on the icon",
            HotkeyAction::ToggleAlwaysHiddenSection => "Like an Option-click on the icon",
            HotkeyAction::EnableIceBar => "Switches Use Ice Bar",
        }
    }

    fn index(self) -> usize {
        match self {
            HotkeyAction::ToggleHiddenSection => 0,
            HotkeyAction::ToggleAlwaysHiddenSection => 1,
            HotkeyAction::EnableIceBar => 2,
        }
    }

    /// The id in the Carbon hotkey id, from 1.
    pub fn id(self) -> u32 {
        self.index() as u32 + 1
    }

    pub fn from_id(id: u32) -> Option<HotkeyAction> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

// Carbon modifier flags (Events.h).
const CARBON_COMMAND: u32 = 1 << 8;
const CARBON_SHIFT: u32 = 1 << 9;
const CARBON_OPTION: u32 = 1 << 11;
const CARBON_CONTROL: u32 = 1 << 12;

// Ice's modifier bits.
const ICE_CONTROL: u64 = 1;
const ICE_OPTION: u64 = 2;
const ICE_SHIFT: u64 = 4;
const ICE_COMMAND: u64 = 8;

/// A key with its modifiers. `key_code` is the virtual key code, which
/// names a key position, not a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub key_code: u16,
    pub modifiers: Modifiers,
}

impl KeyCombo {
    pub fn carbon_modifiers(&self) -> u32 {
        let m = self.modifiers;
        [
            (m.command, CARBON_COMMAND),
            (m.shift, CARBON_SHIFT),
            (m.option, CARBON_OPTION),
            (m.control, CARBON_CONTROL),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |flags, (_, bit)| flags | bit)
    }

    pub fn from_carbon(key_code: u16, flags: u32) -> KeyCombo {
        KeyCombo {
            key_code,
            modifiers: Modifiers {
                shift: flags & CARBON_SHIFT != 0,
                control: flags & CARBON_CONTROL != 0,
                option: flags & CARBON_OPTION != 0,
                command: flags & CARBON_COMMAND != 0,
            },
        }
    }

    fn ice_bits(&self) -> u64 {
        let m = self.modifiers;
        [
            (m.control, ICE_CONTROL),
            (m.option, ICE_OPTION),
            (m.shift, ICE_SHIFT),
            (m.command, ICE_COMMAND),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |bits, (_, bit)| bits | bit)
    }

    fn from_ice_bits(key_code: u16, bits: u64) -> KeyCombo {
        KeyCombo {
            key_code,
            modifiers: Modifiers {
                shift: bits & ICE_SHIFT != 0,
                control: bits & ICE_CONTROL != 0,
                option: bits & ICE_OPTION != 0,
                command: bits & ICE_COMMAND != 0,
            },
        }
    }

    /// The modifier symbols in Apple's order, then `key_name`: "⌃⌥⇧⌘H".
    pub fn display(&self, key_name: &str) -> String {
        let m = self.modifiers;
        let mut out = String::new();
        for (on, symbol) in [
            (m.control, '⌃'),
            (m.option, '⌥'),
            (m.shift, '⇧'),
            (m.command, '⌘'),
        ] {
            if on {
                out.push(symbol);
            }
        }
        out.push_str(key_name);
        out
    }
}

/// The assigned hotkeys, one per action at most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hotkeys([Option<KeyCombo>; 3]);

impl Hotkeys {
    pub fn get(&self, action: HotkeyAction) -> Option<KeyCombo> {
        self.0[action.index()]
    }

    pub fn set(&mut self, action: HotkeyAction, combo: Option<KeyCombo>) {
        self.0[action.index()] = combo;
    }

    pub fn assigned(&self) -> impl Iterator<Item = (HotkeyAction, KeyCombo)> + '_ {
        HotkeyAction::ALL
            .into_iter()
            .filter_map(|a| self.get(a).map(|c| (a, c)))
    }

    /// `{"ToggleHiddenSection": [4, 3]}`; unassigned actions are left out.
    pub fn encode(&self) -> String {
        let map: Map<String, Value> = self
            .assigned()
            .map(|(action, combo)| {
                (
                    action.key().to_string(),
                    Value::from(vec![u64::from(combo.key_code), combo.ice_bits()]),
                )
            })
            .collect();
        Value::Object(map).to_string()
    }

    /// Reads `encode`'s form; anything unreadable is left unassigned.
    pub fn decode(text: &str) -> Hotkeys {
        let mut out = Hotkeys::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return out;
        };
        for (key, value) in map {
            let (Some(action), Some(pair)) = (HotkeyAction::from_key(&key), value.as_array())
            else {
                continue;
            };
            let numbers: Vec<u64> = pair.iter().filter_map(Value::as_u64).collect();
            if let [code, bits] = numbers[..]
                && let Ok(code) = u16::try_from(code)
            {
                out.set(action, Some(KeyCombo::from_ice_bits(code, bits)));
            }
        }
        out
    }
}

/// Why a combination cannot be a hotkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComboProblem {
    NoModifier,
    ShiftOnly,
    /// From macOS 15 on, Carbon does not deliver hotkeys whose only
    /// modifiers are Option or Option-Shift.
    OptionOnly,
    /// An enabled system shortcut (System Settings > Keyboard > Keyboard
    /// Shortcuts).
    System,
    /// Already assigned to another action.
    Taken(HotkeyAction),
}

impl fmt::Display for ComboProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComboProblem::NoModifier => f.write_str("Add Command or Control"),
            ComboProblem::ShiftOnly => f.write_str("Shift alone is not enough"),
            ComboProblem::OptionOnly => {
                f.write_str("macOS does not pass on Option-only shortcuts; add Command or Control")
            }
            ComboProblem::System => f.write_str("macOS uses this shortcut"),
            ComboProblem::Taken(action) => write!(f, "Already used to {}", action_phrase(*action)),
        }
    }
}

fn action_phrase(action: HotkeyAction) -> &'static str {
    match action {
        HotkeyAction::ToggleHiddenSection => "toggle the hidden section",
        HotkeyAction::ToggleAlwaysHiddenSection => "toggle the always-hidden section",
        HotkeyAction::EnableIceBar => "turn the Ice Bar on or off",
    }
}

/// Whether `combo` can be the hotkey of `action`: Ice's rules (a modifier
/// other than Shift, not a system shortcut) plus the macOS 15 rule and no
/// second action on one combination.
pub fn check_combo(
    combo: KeyCombo,
    action: HotkeyAction,
    assigned: &Hotkeys,
    os_major: u32,
    is_system: impl Fn(&KeyCombo) -> bool,
) -> Result<(), ComboProblem> {
    let m = combo.modifiers;
    if !(m.shift || m.control || m.option || m.command) {
        return Err(ComboProblem::NoModifier);
    }
    if !(m.control || m.option || m.command) {
        return Err(ComboProblem::ShiftOnly);
    }
    if os_major >= 15 && m.option && !m.control && !m.command {
        return Err(ComboProblem::OptionOnly);
    }
    if let Some((other, _)) = assigned
        .assigned()
        .find(|(other, existing)| *other != action && *existing == combo)
    {
        return Err(ComboProblem::Taken(other));
    }
    if is_system(&combo) {
        return Err(ComboProblem::System);
    }
    Ok(())
}

/// Virtual key codes of the modifier keys themselves, which never make a
/// hotkey on their own.
pub fn is_modifier_key(key_code: u16) -> bool {
    matches!(key_code, 0x36..=0x3F)
}

/// While recording: Escape cancels, Delete clears.
pub const KEY_ESCAPE: u16 = 0x35;
pub const KEY_DELETE: u16 = 0x33;

/// Names of keys that do not type a character (Events.h key codes).
pub fn special_key_name(key_code: u16) -> Option<&'static str> {
    Some(match key_code {
        0x24 => "↩",
        0x30 => "⇥",
        0x31 => "Space",
        0x33 => "⌫",
        0x35 => "⎋",
        0x4C => "⌤",
        0x72 => "Help",
        0x73 => "↖",
        0x74 => "⇞",
        0x75 => "⌦",
        0x77 => "↘",
        0x79 => "⇟",
        0x7B => "←",
        0x7C => "→",
        0x7D => "↓",
        0x7E => "↑",
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        0x69 => "F13",
        0x6B => "F14",
        0x71 => "F15",
        0x6A => "F16",
        0x40 => "F17",
        0x4F => "F18",
        0x50 => "F19",
        0x5A => "F20",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(code: u16, control: bool, option: bool, shift: bool, command: bool) -> KeyCombo {
        KeyCombo {
            key_code: code,
            modifiers: Modifiers {
                shift,
                control,
                option,
                command,
            },
        }
    }

    #[test]
    fn actions_round_trip() {
        for action in HotkeyAction::ALL {
            assert_eq!(HotkeyAction::from_key(action.key()), Some(action));
            assert_eq!(HotkeyAction::from_id(action.id()), Some(action));
        }
        assert_eq!(HotkeyAction::from_id(0), None);
        assert_eq!(HotkeyAction::from_key("SearchMenuBarItems"), None);
    }

    #[test]
    fn modifiers_convert_both_ways() {
        let c = combo(4, true, true, false, false);
        assert_eq!(c.carbon_modifiers(), CARBON_CONTROL | CARBON_OPTION);
        assert_eq!(KeyCombo::from_carbon(4, c.carbon_modifiers()), c);
        assert_eq!(c.ice_bits(), 3);
        assert_eq!(KeyCombo::from_ice_bits(4, 3), c);
        let all = combo(4, true, true, true, true);
        assert_eq!(all.ice_bits(), 15);
        assert_eq!(all.carbon_modifiers(), 0x1B00);
        assert_eq!(all.display("H"), "⌃⌥⇧⌘H");
        assert_eq!(combo(0x7A, false, false, false, true).display("F1"), "⌘F1");
    }

    #[test]
    fn stored_form_is_ices() {
        let mut keys = Hotkeys::default();
        keys.set(
            HotkeyAction::ToggleHiddenSection,
            Some(combo(4, true, true, false, false)),
        );
        let text = keys.encode();
        assert_eq!(text, r#"{"ToggleHiddenSection":[4,3]}"#);
        assert_eq!(Hotkeys::decode(&text), keys);
        // Ice writes null for an unassigned action; junk is ignored.
        let read = Hotkeys::decode(
            r#"{"ToggleHiddenSection":null,"EnableIceBar":[11,9],"Bogus":[1,1],"ToggleAlwaysHiddenSection":[70000,1]}"#,
        );
        assert_eq!(read.get(HotkeyAction::ToggleHiddenSection), None);
        assert_eq!(
            read.get(HotkeyAction::EnableIceBar),
            Some(combo(11, true, false, false, true))
        );
        assert_eq!(read.get(HotkeyAction::ToggleAlwaysHiddenSection), None);
        assert_eq!(Hotkeys::decode("not json"), Hotkeys::default());
        assert_eq!(Hotkeys::default().encode(), "{}");
    }

    #[test]
    fn combo_rules() {
        let none = |_: &KeyCombo| false;
        let empty = Hotkeys::default();
        let hidden = HotkeyAction::ToggleHiddenSection;
        let check = |c: KeyCombo, os: u32| check_combo(c, hidden, &empty, os, none);
        assert_eq!(
            check(combo(4, false, false, false, false), 26),
            Err(ComboProblem::NoModifier)
        );
        assert_eq!(
            check(combo(4, false, false, true, false), 26),
            Err(ComboProblem::ShiftOnly)
        );
        // Option and Option-Shift alone: refused from macOS 15 on.
        assert_eq!(
            check(combo(4, false, true, false, false), 26),
            Err(ComboProblem::OptionOnly)
        );
        assert_eq!(
            check(combo(4, false, true, true, false), 15),
            Err(ComboProblem::OptionOnly)
        );
        assert_eq!(check(combo(4, false, true, false, false), 14), Ok(()));
        assert_eq!(check(combo(4, true, true, false, false), 26), Ok(()));
        assert_eq!(check(combo(4, false, true, false, true), 26), Ok(()));
        // A system shortcut, and one already used by another action.
        let ctrl_space = combo(0x31, true, false, false, false);
        assert_eq!(
            check_combo(ctrl_space, hidden, &empty, 26, |c| *c == ctrl_space),
            Err(ComboProblem::System)
        );
        let mut assigned = Hotkeys::default();
        let ctrl_opt_h = combo(4, true, true, false, false);
        assigned.set(HotkeyAction::EnableIceBar, Some(ctrl_opt_h));
        assert_eq!(
            check_combo(ctrl_opt_h, hidden, &assigned, 26, none),
            Err(ComboProblem::Taken(HotkeyAction::EnableIceBar))
        );
        // Recording the same combination again for its own action is fine.
        assert_eq!(
            check_combo(ctrl_opt_h, HotkeyAction::EnableIceBar, &assigned, 26, none),
            Ok(())
        );
        assert_eq!(
            ComboProblem::Taken(HotkeyAction::EnableIceBar).to_string(),
            "Already used to turn the Ice Bar on or off"
        );
    }

    #[test]
    fn key_names() {
        assert_eq!(special_key_name(0x31), Some("Space"));
        assert_eq!(special_key_name(0x7A), Some("F1"));
        assert_eq!(special_key_name(0x7E), Some("↑"));
        assert_eq!(special_key_name(4), None);
        assert!(is_modifier_key(0x37));
        assert!(is_modifier_key(0x3A));
        assert!(!is_modifier_key(4));
    }
}
