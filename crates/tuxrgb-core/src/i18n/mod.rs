//! Interface translations.
//!
//! TuxRGB is written in French: every user-facing string in the source is
//! the French text, which is also the lookup key (like gettext msgids).
//! [`tr`] returns the translation for the current language, or the French
//! original when there is none, so a missing entry never shows an empty
//! label. Formatted strings use positional `{}` placeholders and go through
//! [`fill`] (see the `trf!` macro of the application).
//!
//! The language is detected from the environment (`LC_ALL`, then
//! `LC_MESSAGES`, `LANG`, `LANGUAGE`) and can be forced in the settings; it
//! is a process-wide setting read lock-free on every lookup.

mod en;

use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Fr, Lang::En];

    /// Name of the language in that language (for the settings).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Fr => "Français",
            Lang::En => "English",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Fr => "fr",
            Lang::En => "en",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Fr => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn language() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Fr,
    }
}

/// Language of the user session. French locales (`fr_FR`, `fr_CA`,
/// `fr_BE.UTF-8`…) give French; anything else gives English, the most
/// widely understood fallback.
pub fn system_language() -> Lang {
    language_from_env(|k| std::env::var(k).ok())
}

/// Testable core of [`system_language`].
pub fn language_from_env(get: impl Fn(&str) -> Option<String>) -> Lang {
    // POSIX precedence; LANGUAGE is a priority list ("fr:en_US").
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(v) = get(key).filter(|v| !v.is_empty()) {
            if v == "C" || v == "POSIX" || v.starts_with("C.") {
                // An explicit "C" locale: check LANGUAGE before giving up.
                break;
            }
            return from_tag(&v);
        }
    }
    get("LANGUAGE")
        .and_then(|list| list.split(':').find(|t| !t.is_empty()).map(from_tag))
        .unwrap_or(Lang::En)
}

fn from_tag(tag: &str) -> Lang {
    let t = tag.trim().to_ascii_lowercase();
    if t.starts_with("fr") {
        Lang::Fr
    } else {
        Lang::En
    }
}

fn english() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| en::ENTRIES.iter().copied().collect())
}

/// Translates a French source string into the current language.
#[inline]
pub fn tr(s: &str) -> &str {
    match language() {
        Lang::Fr => s,
        Lang::En => english().get(s).copied().unwrap_or(s),
    }
}

/// Translates into a given language (tray, tests).
pub fn tr_in(lang: Lang, s: &str) -> &str {
    match lang {
        Lang::Fr => s,
        Lang::En => english().get(s).copied().unwrap_or(s),
    }
}

/// Replaces each `{}` of an (already translated) template by the next
/// argument. `{{` / `}}` are literal braces.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16 * args.len());
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' if chars.peek() == Some(&'}') => {
                chars.next();
                if let Some(a) = args.next() {
                    let _ = write!(out, "{a}");
                }
            }
            c => out.push(c),
        }
    }
    if language() == Lang::En {
        singular_led(&mut out);
    }
    out
}

/// English plural: "{} LEDs" filled with exactly 1 reads "1 LED".
fn singular_led(s: &mut String) {
    let mut from = 0;
    while let Some(i) = s[from..].find("1 LEDs").map(|i| i + from) {
        let lone = i == 0 || !s.as_bytes()[i - 1].is_ascii_digit();
        let end = i + "1 LEDs".len();
        let word_end = s
            .as_bytes()
            .get(end)
            .is_none_or(|b| !b.is_ascii_alphanumeric());
        if lone && word_end {
            s.remove(end - 1);
        }
        from = end - 1;
    }
}

/// Every French key that has an English translation (tooling and tests).
pub fn english_keys() -> impl Iterator<Item = &'static str> {
    en::ENTRIES.iter().map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn detects_the_session_language() {
        assert_eq!(language_from_env(env(&[("LANG", "fr_FR.UTF-8")])), Lang::Fr);
        assert_eq!(language_from_env(env(&[("LANG", "en_US.UTF-8")])), Lang::En);
        assert_eq!(language_from_env(env(&[("LANG", "de_DE.UTF-8")])), Lang::En);
        // LC_ALL wins over LANG, LC_MESSAGES over LANG.
        assert_eq!(
            language_from_env(env(&[("LC_ALL", "en_GB.UTF-8"), ("LANG", "fr_FR.UTF-8")])),
            Lang::En
        );
        assert_eq!(
            language_from_env(env(&[("LC_MESSAGES", "fr_CA.UTF-8"), ("LANG", "en_US")])),
            Lang::Fr
        );
        // "C" falls back to LANGUAGE.
        assert_eq!(
            language_from_env(env(&[("LANG", "C.UTF-8"), ("LANGUAGE", "fr:en")])),
            Lang::Fr
        );
        assert_eq!(language_from_env(env(&[])), Lang::En);
    }

    #[test]
    fn one_led_is_singular_in_english() {
        let mut s = "Single LED · 1 LEDs, 11 LEDs, 21 LEDs, 1 LEDs".to_owned();
        singular_led(&mut s);
        assert_eq!(s, "Single LED · 1 LED, 11 LEDs, 21 LEDs, 1 LED");
        let mut s = "1 LEDstrip".to_owned();
        singular_led(&mut s);
        assert_eq!(s, "1 LEDstrip");
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{} LED sur {}", &[&3, &"RAM"]), "3 LED sur RAM");
        assert_eq!(fill("{{x}} {}", &[&1]), "{x} 1");
        assert_eq!(fill("{} et {}", &[&1]), "1 et ");
    }

    #[test]
    fn english_dictionary_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for (fr, en) in en::ENTRIES {
            assert!(seen.insert(*fr), "duplicate key: {fr}");
            assert!(!en.is_empty(), "empty translation for {fr}");
            assert_eq!(
                fr.matches("{}").count(),
                en.matches("{}").count(),
                "placeholder count differs: {fr} → {en}"
            );
        }
        assert_eq!(tr_in(Lang::En, "Statique"), "Static");
        assert_eq!(tr_in(Lang::Fr, "Statique"), "Statique");
        assert_eq!(
            tr_in(Lang::En, "pas dans le dictionnaire"),
            "pas dans le dictionnaire"
        );
    }
}
