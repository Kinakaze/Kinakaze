//! Translating Windows key messages into Linux evdev keycodes.
//!
//! A guest expects Linux keycodes — the `KEY_*` numbers from
//! `linux/input-event-codes.h` — because that is what every Linux input path
//! reports, from `libinput` up through SDL and GLFW. Handing it Windows virtual-key
//! codes would work only for guest code rewritten to know it is on Windows, which
//! defeats the point.
//!
//! ## Why the scancode, and not the virtual-key code
//!
//! The obvious translation is a table from `VK_*` to `KEY_*`. It is the wrong one,
//! for a reason worth stating: **virtual-key codes are layout-dependent.** The key
//! to the right of `L` is `VK_OEM_1` on a US layout and something else on a German
//! one, so a `VK_*` table silently mistranslates every non-US keyboard. Windows
//! also reports plain `VK_SHIFT` for both shift keys, while Linux distinguishes
//! `KEY_LEFTSHIFT` from `KEY_RIGHTSHIFT`, so the table cannot even express the
//! distinction.
//!
//! The scancode in `lParam` bits 16-23 is layout-independent: it names the physical
//! key. And there is a structural fact that makes it more than merely better —
//! **Linux evdev keycodes for the main keyboard block are numerically equal to
//! PS/2 set-1 scancodes.** Escape is scancode 1 and `KEY_ESC` is 1; `A` is scancode
//! 0x1E = 30 and `KEY_A` is 30; `F10` is scancode 0x44 = 68 and `KEY_F10` is 68.
//! That is not a coincidence: Linux numbered its keycodes from the AT keyboard's
//! scancodes. The identity holds unbroken from 0x01 to 0x58, which is every key in
//! the main block, and is verified as a property by the tests below rather than
//! asserted key by key.
//!
//! So the translation is the identity for the main block, plus one small table for
//! the keys Windows marks *extended*. Those are the `E0`-prefixed scancodes — the
//! duplicated keys added by the 101-key keyboard — where the same scancode means
//! two different keys depending on the prefix, and Linux gives each its own
//! number.

/// A Linux evdev keycode.
pub type Keycode = u16;

/// The highest scancode for which evdev numbering equals set-1 numbering.
///
/// 0x58 is F12. Above it the set-1 map has gaps that Linux fills differently, so
/// nothing beyond this is translated by identity.
const IDENTITY_LIMIT: u16 = 0x58;

/// `KEY_RESERVED`: no key. Returned when a scancode cannot be translated, which is
/// the value Linux itself uses for "nothing here".
pub const KEY_RESERVED: Keycode = 0;

/// The extended scancodes, paired with their evdev keycodes.
///
/// Windows reports these with bit 24 of `lParam` set, standing in for the `E0`
/// prefix the keyboard actually sends. Each collides with a main-block scancode —
/// `E0 1C` is keypad Enter where `1C` is the main Enter — so the extended bit is
/// what distinguishes them and cannot be ignored.
const EXTENDED: &[(u16, Keycode)] = &[
    (0x1C, 96),  // KEY_KPENTER
    (0x1D, 97),  // KEY_RIGHTCTRL
    (0x35, 98),  // KEY_KPSLASH
    (0x37, 99),  // KEY_SYSRQ, the Print Screen key
    (0x38, 100), // KEY_RIGHTALT
    (0x47, 102), // KEY_HOME
    (0x48, 103), // KEY_UP
    (0x49, 104), // KEY_PAGEUP
    (0x4B, 105), // KEY_LEFT
    (0x4D, 106), // KEY_RIGHT
    (0x4F, 107), // KEY_END
    (0x50, 108), // KEY_DOWN
    (0x51, 109), // KEY_PAGEDOWN
    (0x52, 110), // KEY_INSERT
    (0x53, 111), // KEY_DELETE
    (0x5B, 125), // KEY_LEFTMETA
    (0x5C, 126), // KEY_RIGHTMETA
    (0x5D, 127), // KEY_COMPOSE, the menu key
];

/// The non-extended scancodes above [`IDENTITY_LIMIT`] that still have a keycode.
///
/// Pause is the one key Windows reports non-extended above the identity range: the
/// hardware sends the three-byte sequence `E1 1D 45`, and Windows surfaces it as
/// scancode 0x45 with the extended bit clear — which is also NumLock's scancode.
/// They are distinguished by the virtual-key code, so this table is consulted only
/// when that has already been checked.
const TAIL: &[(u16, Keycode)] = &[
    (0x45, 119), // KEY_PAUSE
];

/// `VK_PAUSE`, needed to separate Pause from NumLock. See [`TAIL`].
const VK_PAUSE: u16 = 0x13;
/// `VK_NUMLOCK`, the other reading of scancode 0x45.
const VK_NUMLOCK: u16 = 0x90;

/// Translates one Windows key message into a Linux keycode.
///
/// `scancode` is `lParam` bits 16-23, `extended` is bit 24, and `virtual_key` is
/// `wParam`. The virtual-key code is used only to break the one ambiguity the
/// scancode cannot: see [`TAIL`].
///
/// Returns [`KEY_RESERVED`] for a scancode with no Linux equivalent, which a
/// caller should drop rather than report — a zero keycode means "no key" to every
/// Linux consumer, so passing it on is harmless but meaningless.
pub fn translate(scancode: u16, extended: bool, virtual_key: u16) -> Keycode {
    if extended {
        return EXTENDED
            .iter()
            .find(|(code, _)| *code == scancode)
            .map_or(KEY_RESERVED, |(_, keycode)| *keycode);
    }

    // Pause and NumLock share scancode 0x45 with the extended bit clear.
    if scancode == 0x45 {
        return match virtual_key {
            VK_PAUSE => 119,
            VK_NUMLOCK => 69,
            // An unfamiliar virtual key on this scancode is more likely NumLock,
            // which is what the identity range would have said anyway.
            _ => 69,
        };
    }

    if (1..=IDENTITY_LIMIT).contains(&scancode) {
        return scancode;
    }

    TAIL.iter()
        .find(|(code, _)| *code == scancode)
        .map_or(KEY_RESERVED, |(_, keycode)| *keycode)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structural claim the module rests on, checked as a property rather than
    /// key by key: through 0x58, a non-extended scancode *is* its keycode.
    ///
    /// If this ever fails, the identity shortcut is wrong and the module needs a
    /// full table — so it is worth testing the whole range rather than samples.
    #[test]
    fn the_main_block_is_the_identity() {
        for scancode in 1..=IDENTITY_LIMIT {
            // 0x45 is the documented exception, resolved by virtual key.
            if scancode == 0x45 {
                continue;
            }
            assert_eq!(
                translate(scancode, false, 0),
                scancode,
                "scancode {scancode:#x} should translate to itself"
            );
        }
    }

    /// Spot-checks against `linux/input-event-codes.h` values written out by hand,
    /// so the identity property above is anchored to real numbers and not just to
    /// itself.
    #[test]
    fn known_keys_have_their_documented_keycodes() {
        // (scancode, extended, expected KEY_* value, name)
        let cases = [
            (0x01, false, 1, "KEY_ESC"),
            (0x02, false, 2, "KEY_1"),
            (0x0E, false, 14, "KEY_BACKSPACE"),
            (0x0F, false, 15, "KEY_TAB"),
            (0x10, false, 16, "KEY_Q"),
            (0x1C, false, 28, "KEY_ENTER"),
            (0x1D, false, 29, "KEY_LEFTCTRL"),
            (0x1E, false, 30, "KEY_A"),
            (0x2A, false, 42, "KEY_LEFTSHIFT"),
            (0x2C, false, 44, "KEY_Z"),
            (0x36, false, 54, "KEY_RIGHTSHIFT"),
            (0x38, false, 56, "KEY_LEFTALT"),
            (0x39, false, 57, "KEY_SPACE"),
            (0x3B, false, 59, "KEY_F1"),
            (0x44, false, 68, "KEY_F10"),
            (0x58, false, 88, "KEY_F12"),
        ];
        for (scancode, extended, expected, name) in cases {
            assert_eq!(
                translate(scancode, extended, 0),
                expected,
                "{name} (scancode {scancode:#x})"
            );
        }
    }

    /// The extended keys are the reason the extended bit cannot be dropped: each
    /// shares a scancode with a different main-block key.
    #[test]
    fn extended_keys_are_distinct_from_their_collisions() {
        // Enter versus keypad Enter.
        assert_eq!(translate(0x1C, false, 0), 28);
        assert_eq!(translate(0x1C, true, 0), 96);
        // Left versus right control.
        assert_eq!(translate(0x1D, false, 0), 29);
        assert_eq!(translate(0x1D, true, 0), 97);
        // Left versus right alt.
        assert_eq!(translate(0x38, false, 0), 56);
        assert_eq!(translate(0x38, true, 0), 100);
        // Main-block slash versus keypad slash.
        assert_eq!(translate(0x35, false, 0), 53);
        assert_eq!(translate(0x35, true, 0), 98);
    }

    /// The arrow keys and the navigation block, which every interactive guest uses.
    #[test]
    fn the_navigation_block_translates() {
        assert_eq!(translate(0x48, true, 0), 103, "KEY_UP");
        assert_eq!(translate(0x50, true, 0), 108, "KEY_DOWN");
        assert_eq!(translate(0x4B, true, 0), 105, "KEY_LEFT");
        assert_eq!(translate(0x4D, true, 0), 106, "KEY_RIGHT");
        assert_eq!(translate(0x47, true, 0), 102, "KEY_HOME");
        assert_eq!(translate(0x4F, true, 0), 107, "KEY_END");
        assert_eq!(translate(0x52, true, 0), 110, "KEY_INSERT");
        assert_eq!(translate(0x53, true, 0), 111, "KEY_DELETE");
    }

    /// Numpad digits are *not* the same keys as the main-row digits, and Linux
    /// numbers them separately. The identity range covers them, so this checks the
    /// range extends far enough.
    #[test]
    fn the_numpad_is_separate_from_the_number_row() {
        assert_eq!(translate(0x47, false, 0), 71, "KEY_KP7");
        assert_eq!(translate(0x52, false, 0), 82, "KEY_KP0");
        assert_eq!(translate(0x53, false, 0), 83, "KEY_KPDOT");
        // The main-row 7, for contrast.
        assert_eq!(translate(0x08, false, 0), 8, "KEY_7");
    }

    /// Scancode 0x45 is the one place the virtual-key code has to be consulted.
    #[test]
    fn pause_and_numlock_are_separated_by_virtual_key() {
        assert_eq!(translate(0x45, false, VK_PAUSE), 119, "KEY_PAUSE");
        assert_eq!(translate(0x45, false, VK_NUMLOCK), 69, "KEY_NUMLOCK");
    }

    /// An unknown scancode must report "no key" rather than a plausible wrong one.
    #[test]
    fn unknown_scancodes_are_reserved() {
        assert_eq!(translate(0, false, 0), KEY_RESERVED);
        assert_eq!(translate(0x7F, false, 0), KEY_RESERVED);
        assert_eq!(translate(0x0A, true, 0), KEY_RESERVED);
    }

    /// No two entries in the extended table may share a keycode, which would mean
    /// two physical keys reported as one.
    #[test]
    fn the_extended_table_is_unambiguous() {
        for (index, (scancode, keycode)) in EXTENDED.iter().enumerate() {
            for (other_scancode, other_keycode) in &EXTENDED[index + 1..] {
                assert_ne!(scancode, other_scancode, "duplicate scancode {scancode:#x}");
                assert_ne!(keycode, other_keycode, "duplicate keycode {keycode}");
            }
        }
    }
}
