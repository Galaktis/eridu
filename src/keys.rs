//! Letter and symbol shortcuts go by the character typed, not by the key's position: Bevy's
//! `KeyCode` is the physical (QWERTY) position, so on AZERTY `KeyCode::KeyQ` is the A key.
//! Arrows, Space, F-keys, Home and Esc are the same everywhere and stay `KeyCode`s.

use bevy::input::ButtonInput;
use bevy::input::keyboard::Key;

/// The key for character `c` (either case) was just pressed.
pub fn typed(keys: &ButtonInput<Key>, c: &str) -> bool {
    keys.get_just_pressed()
        .any(|key| matches!(key, Key::Character(s) if s.eq_ignore_ascii_case(c)))
}

/// A key for one of `chars` is held down.
pub fn holding(keys: &ButtonInput<Key>, chars: &[&str]) -> bool {
    keys.get_pressed()
        .any(|key| matches!(key, Key::Character(s) if chars.contains(&s.as_str())))
}
