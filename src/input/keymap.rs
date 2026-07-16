use crate::protocol::KeyCode;

use super::InputError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub shift: bool,
}

pub fn map_text(text: &str) -> Result<Vec<KeyStroke>, InputError> {
    text.chars().map(map_character).collect()
}

fn map_character(character: char) -> Result<KeyStroke, InputError> {
    use KeyCode::{
        Apostrophe, Backslash, Comma, Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6,
        Digit7, Digit8, Digit9, Enter, Equal, Grave, KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG,
        KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV,
        KeyW, KeyX, KeyY, KeyZ, LeftBracket, Minus, Period, RightBracket, Semicolon, Slash, Space,
        Tab,
    };
    let (code, shift) = match character {
        'a'..='z' => {
            let code = [
                KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN,
                KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
            ][usize::from(character as u8 - b'a')];
            (code, false)
        }
        'A'..='Z' => {
            let code = [
                KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN,
                KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
            ][usize::from(character as u8 - b'A')];
            (code, true)
        }
        '1'..='9' => {
            let code = [
                Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
            ][usize::from(character as u8 - b'1')];
            (code, false)
        }
        '0' => (Digit0, false),
        '!' => (Digit1, true),
        '@' => (Digit2, true),
        '#' => (Digit3, true),
        '$' => (Digit4, true),
        '%' => (Digit5, true),
        '^' => (Digit6, true),
        '&' => (Digit7, true),
        '*' => (Digit8, true),
        '(' => (Digit9, true),
        ')' => (Digit0, true),
        ' ' => (Space, false),
        '-' => (Minus, false),
        '_' => (Minus, true),
        '=' => (Equal, false),
        '+' => (Equal, true),
        '[' => (LeftBracket, false),
        '{' => (LeftBracket, true),
        ']' => (RightBracket, false),
        '}' => (RightBracket, true),
        '\\' => (Backslash, false),
        '|' => (Backslash, true),
        ';' => (Semicolon, false),
        ':' => (Semicolon, true),
        '\'' => (Apostrophe, false),
        '"' => (Apostrophe, true),
        '`' => (Grave, false),
        '~' => (Grave, true),
        ',' => (Comma, false),
        '<' => (Comma, true),
        '.' => (Period, false),
        '>' => (Period, true),
        '/' => (Slash, false),
        '?' => (Slash, true),
        '\n' => (Enter, false),
        '\t' => (Tab, false),
        _ => return Err(InputError::UnsupportedCharacter(character)),
    };
    Ok(KeyStroke { code, shift })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_us_shift_combinations() {
        assert_eq!(
            map_text("aA!?_").unwrap(),
            vec![
                KeyStroke {
                    code: KeyCode::KeyA,
                    shift: false
                },
                KeyStroke {
                    code: KeyCode::KeyA,
                    shift: true
                },
                KeyStroke {
                    code: KeyCode::Digit1,
                    shift: true
                },
                KeyStroke {
                    code: KeyCode::Slash,
                    shift: true
                },
                KeyStroke {
                    code: KeyCode::Minus,
                    shift: true
                },
            ]
        );
    }

    #[test]
    fn rejects_unicode_before_emitting() {
        assert!(matches!(
            map_text("oké"),
            Err(InputError::UnsupportedCharacter('é'))
        ));
    }
}
