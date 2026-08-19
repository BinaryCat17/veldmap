//! Конвертация proto KeyEvent в события клавиатуры iced.
//!
//! Хост шлёт физический код (дискриминант winit KeyCode), текст нажатия
//! (раскладка уже применена) и маску модификаторов. text_input'у iced нужны
//! logical key (Named для служебных, Character для печатных), text и
//! ModifiersChanged — modifiers он хранит у себя и обновляет только по нему.

use iced_core::keyboard::{self, key, Key, Location, Modifiers};
use iced_core::SmolStr;
use veldsdk::proto::app::KeyEvent;

// Биты маски KeyEvent.modifiers (см. app.proto).
const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_ALT: u32 = 4;
const MOD_SUPER: u32 = 8;

pub fn modifiers_from_bits(bits: u32) -> Modifiers {
    let mut m = Modifiers::empty();
    if bits & MOD_SHIFT != 0 { m |= Modifiers::SHIFT; }
    if bits & MOD_CTRL != 0 { m |= Modifiers::CTRL; }
    if bits & MOD_ALT != 0 { m |= Modifiers::ALT; }
    if bits & MOD_SUPER != 0 { m |= Modifiers::LOGO; }
    m
}

/// Событие клавиатуры iced из proto KeyEvent.
pub fn convert_key_event(ev: &KeyEvent, modifiers: Modifiers) -> keyboard::Event {
    let code = CODE_TABLE.get(ev.key_code as usize).copied();
    let named = code.and_then(named_from_code);
    let text = if ev.pressed && !ev.text.is_empty() { Some(ev.text.as_str()) } else { None };

    // Логическая клавиша: служебная → Named, печатный текст → Character.
    // Если текст пуст или содержит только control-символы (так выглядит
    // Ctrl+C на Linux: text = "\x03"), для сочетаний с Ctrl/Alt/Super
    // восстанавливаем ASCII-символ из физического кода — иначе хоткеи
    // вида Ctrl+C/X/V/A в text_input не сработают.
    let logical: Key = if let Some(named) = named {
        Key::Named(named)
    } else if let Some(t) = text.filter(|t| !t.chars().next().is_none_or(|c| c.is_control())) {
        Key::Character(SmolStr::from(t))
    } else if modifiers.intersects(Modifiers::CTRL | Modifiers::ALT | Modifiers::LOGO) {
        code.and_then(ascii_from_code)
            .map(|c| Key::Character(SmolStr::from(c)))
            .unwrap_or(Key::Unidentified)
    } else {
        Key::Unidentified
    };

    let location = match code {
        Some(c) if is_numpad(c) => Location::Numpad,
        _ => Location::Standard,
    };

    let physical_key = code.map(key::Physical::Code)
        .unwrap_or(key::Physical::Unidentified(key::NativeCode::Xkb(ev.key_code)));

    if ev.pressed {
        keyboard::Event::KeyPressed {
            modified_key: logical.clone(),
            key: logical,
            physical_key,
            location,
            modifiers,
            text: text.map(SmolStr::from),
            // Автоповтор хост не различает: KeyEvent несёт только код, текст и
            // модификаторы (см. app.proto). Все нажатия приходят как первые.
            repeat: false,
        }
    } else {
        keyboard::Event::KeyReleased {
            modified_key: logical.clone(),
            key: logical,
            physical_key,
            location,
            modifiers,
        }
    }
}

/// Named-эквивалент служебных клавиш по физическому коду.
fn named_from_code(code: key::Code) -> Option<key::Named> {
    use key::{Code, Named};
    Some(match code {
        Code::Enter | Code::NumpadEnter => Named::Enter,
        Code::Tab => Named::Tab,
        Code::Space => Named::Space,
        Code::ArrowDown => Named::ArrowDown,
        Code::ArrowLeft => Named::ArrowLeft,
        Code::ArrowRight => Named::ArrowRight,
        Code::ArrowUp => Named::ArrowUp,
        Code::End => Named::End,
        Code::Home => Named::Home,
        Code::PageDown => Named::PageDown,
        Code::PageUp => Named::PageUp,
        Code::Backspace => Named::Backspace,
        Code::Delete => Named::Delete,
        Code::Insert => Named::Insert,
        Code::Escape => Named::Escape,
        Code::PrintScreen => Named::PrintScreen,
        Code::ContextMenu => Named::ContextMenu,
        Code::Pause => Named::Pause,
        Code::ShiftLeft | Code::ShiftRight => Named::Shift,
        Code::ControlLeft | Code::ControlRight => Named::Control,
        Code::AltLeft => Named::Alt,
        Code::AltRight => Named::AltGraph,
        Code::SuperLeft | Code::SuperRight => Named::Super,
        Code::CapsLock => Named::CapsLock,
        Code::NumLock => Named::NumLock,
        Code::ScrollLock => Named::ScrollLock,
        Code::F1 => Named::F1,
        Code::F2 => Named::F2,
        Code::F3 => Named::F3,
        Code::F4 => Named::F4,
        Code::F5 => Named::F5,
        Code::F6 => Named::F6,
        Code::F7 => Named::F7,
        Code::F8 => Named::F8,
        Code::F9 => Named::F9,
        Code::F10 => Named::F10,
        Code::F11 => Named::F11,
        Code::F12 => Named::F12,
        _ => return None,
    })
}

/// US-ASCII символ клавиши — fallback для хоткеев, когда текст нажатия
/// съеден модификаторами (Ctrl+C → "\x03"). Раскладка здесь не важна:
/// хоткеи в iced привязаны к латинице.
fn ascii_from_code(code: key::Code) -> Option<&'static str> {
    use key::Code;
    Some(match code {
        Code::KeyA => "a", Code::KeyB => "b", Code::KeyC => "c", Code::KeyD => "d",
        Code::KeyE => "e", Code::KeyF => "f", Code::KeyG => "g", Code::KeyH => "h",
        Code::KeyI => "i", Code::KeyJ => "j", Code::KeyK => "k", Code::KeyL => "l",
        Code::KeyM => "m", Code::KeyN => "n", Code::KeyO => "o", Code::KeyP => "p",
        Code::KeyQ => "q", Code::KeyR => "r", Code::KeyS => "s", Code::KeyT => "t",
        Code::KeyU => "u", Code::KeyV => "v", Code::KeyW => "w", Code::KeyX => "x",
        Code::KeyY => "y", Code::KeyZ => "z",
        Code::Digit0 => "0", Code::Digit1 => "1", Code::Digit2 => "2", Code::Digit3 => "3",
        Code::Digit4 => "4", Code::Digit5 => "5", Code::Digit6 => "6", Code::Digit7 => "7",
        Code::Digit8 => "8", Code::Digit9 => "9",
        Code::Minus => "-", Code::Equal => "=", Code::Comma => ",", Code::Period => ".",
        Code::Slash => "/", Code::Backslash => "\\", Code::Semicolon => ";", Code::Quote => "'",
        Code::BracketLeft => "[", Code::BracketRight => "]", Code::Backquote => "`",
        _ => return None,
    })
}

fn is_numpad(code: key::Code) -> bool {
    use key::Code;
    matches!(code,
        Code::Numpad0 | Code::Numpad1 | Code::Numpad2 | Code::Numpad3 | Code::Numpad4 |
        Code::Numpad5 | Code::Numpad6 | Code::Numpad7 | Code::Numpad8 | Code::Numpad9 |
        Code::NumpadAdd | Code::NumpadSubtract | Code::NumpadMultiply | Code::NumpadDivide |
        Code::NumpadDecimal | Code::NumpadEnter | Code::NumpadEqual | Code::NumpadComma)
}

// Порядок вариантов совпадает с winit 0.30 `KeyCode`: `key_code` — дискриминант
// winit (его ставит раннер, см. `runners/desktop/src/main.rs`), здесь — индекс в
// таблице. Сверено с обеими декларациями (194 варианта).
//
// Договор этот держится на числовом значении варианта чужого enum'а, и
// сломать его может обновление winit: вставленный вариант сдвинет всю
// клавиатуру, а сборка об этом не скажет ни слова. Закрепить его тестом
// нельзя: нативной сборки у этого модуля нет вовсе — `cargo test` тянет
// иксовые зависимости iced (softbuffer), которые на здешнем nightly не
// собираются. Проверяется он поэтому только клавиатурой: после обновления
// winit убедиться, что буквы приходят буквами.
static CODE_TABLE: &[key::Code] = &[
    key::Code::Backquote,
    key::Code::Backslash,
    key::Code::BracketLeft,
    key::Code::BracketRight,
    key::Code::Comma,
    key::Code::Digit0,
    key::Code::Digit1,
    key::Code::Digit2,
    key::Code::Digit3,
    key::Code::Digit4,
    key::Code::Digit5,
    key::Code::Digit6,
    key::Code::Digit7,
    key::Code::Digit8,
    key::Code::Digit9,
    key::Code::Equal,
    key::Code::IntlBackslash,
    key::Code::IntlRo,
    key::Code::IntlYen,
    key::Code::KeyA,
    key::Code::KeyB,
    key::Code::KeyC,
    key::Code::KeyD,
    key::Code::KeyE,
    key::Code::KeyF,
    key::Code::KeyG,
    key::Code::KeyH,
    key::Code::KeyI,
    key::Code::KeyJ,
    key::Code::KeyK,
    key::Code::KeyL,
    key::Code::KeyM,
    key::Code::KeyN,
    key::Code::KeyO,
    key::Code::KeyP,
    key::Code::KeyQ,
    key::Code::KeyR,
    key::Code::KeyS,
    key::Code::KeyT,
    key::Code::KeyU,
    key::Code::KeyV,
    key::Code::KeyW,
    key::Code::KeyX,
    key::Code::KeyY,
    key::Code::KeyZ,
    key::Code::Minus,
    key::Code::Period,
    key::Code::Quote,
    key::Code::Semicolon,
    key::Code::Slash,
    key::Code::AltLeft,
    key::Code::AltRight,
    key::Code::Backspace,
    key::Code::CapsLock,
    key::Code::ContextMenu,
    key::Code::ControlLeft,
    key::Code::ControlRight,
    key::Code::Enter,
    key::Code::SuperLeft,
    key::Code::SuperRight,
    key::Code::ShiftLeft,
    key::Code::ShiftRight,
    key::Code::Space,
    key::Code::Tab,
    key::Code::Convert,
    key::Code::KanaMode,
    key::Code::Lang1,
    key::Code::Lang2,
    key::Code::Lang3,
    key::Code::Lang4,
    key::Code::Lang5,
    key::Code::NonConvert,
    key::Code::Delete,
    key::Code::End,
    key::Code::Help,
    key::Code::Home,
    key::Code::Insert,
    key::Code::PageDown,
    key::Code::PageUp,
    key::Code::ArrowDown,
    key::Code::ArrowLeft,
    key::Code::ArrowRight,
    key::Code::ArrowUp,
    key::Code::NumLock,
    key::Code::Numpad0,
    key::Code::Numpad1,
    key::Code::Numpad2,
    key::Code::Numpad3,
    key::Code::Numpad4,
    key::Code::Numpad5,
    key::Code::Numpad6,
    key::Code::Numpad7,
    key::Code::Numpad8,
    key::Code::Numpad9,
    key::Code::NumpadAdd,
    key::Code::NumpadBackspace,
    key::Code::NumpadClear,
    key::Code::NumpadClearEntry,
    key::Code::NumpadComma,
    key::Code::NumpadDecimal,
    key::Code::NumpadDivide,
    key::Code::NumpadEnter,
    key::Code::NumpadEqual,
    key::Code::NumpadHash,
    key::Code::NumpadMemoryAdd,
    key::Code::NumpadMemoryClear,
    key::Code::NumpadMemoryRecall,
    key::Code::NumpadMemoryStore,
    key::Code::NumpadMemorySubtract,
    key::Code::NumpadMultiply,
    key::Code::NumpadParenLeft,
    key::Code::NumpadParenRight,
    key::Code::NumpadStar,
    key::Code::NumpadSubtract,
    key::Code::Escape,
    key::Code::Fn,
    key::Code::FnLock,
    key::Code::PrintScreen,
    key::Code::ScrollLock,
    key::Code::Pause,
    key::Code::BrowserBack,
    key::Code::BrowserFavorites,
    key::Code::BrowserForward,
    key::Code::BrowserHome,
    key::Code::BrowserRefresh,
    key::Code::BrowserSearch,
    key::Code::BrowserStop,
    key::Code::Eject,
    key::Code::LaunchApp1,
    key::Code::LaunchApp2,
    key::Code::LaunchMail,
    key::Code::MediaPlayPause,
    key::Code::MediaSelect,
    key::Code::MediaStop,
    key::Code::MediaTrackNext,
    key::Code::MediaTrackPrevious,
    key::Code::Power,
    key::Code::Sleep,
    key::Code::AudioVolumeDown,
    key::Code::AudioVolumeMute,
    key::Code::AudioVolumeUp,
    key::Code::WakeUp,
    key::Code::Meta,
    key::Code::Hyper,
    key::Code::Turbo,
    key::Code::Abort,
    key::Code::Resume,
    key::Code::Suspend,
    key::Code::Again,
    key::Code::Copy,
    key::Code::Cut,
    key::Code::Find,
    key::Code::Open,
    key::Code::Paste,
    key::Code::Props,
    key::Code::Select,
    key::Code::Undo,
    key::Code::Hiragana,
    key::Code::Katakana,
    key::Code::F1,
    key::Code::F2,
    key::Code::F3,
    key::Code::F4,
    key::Code::F5,
    key::Code::F6,
    key::Code::F7,
    key::Code::F8,
    key::Code::F9,
    key::Code::F10,
    key::Code::F11,
    key::Code::F12,
    key::Code::F13,
    key::Code::F14,
    key::Code::F15,
    key::Code::F16,
    key::Code::F17,
    key::Code::F18,
    key::Code::F19,
    key::Code::F20,
    key::Code::F21,
    key::Code::F22,
    key::Code::F23,
    key::Code::F24,
    key::Code::F25,
    key::Code::F26,
    key::Code::F27,
    key::Code::F28,
    key::Code::F29,
    key::Code::F30,
    key::Code::F31,
    key::Code::F32,
    key::Code::F33,
    key::Code::F34,
    key::Code::F35,
];
