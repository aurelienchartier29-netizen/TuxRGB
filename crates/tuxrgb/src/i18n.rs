//! Translation glue for the GUI (see `tuxrgb_core::i18n`).
//!
//! * `&str` passed to the text helpers of `ui::widgets` is a French source
//!   string and is translated on the fly;
//! * a `String` is already final (names of devices, formatted text) and is
//!   shown as is – formatted messages are built with [`trf!`], which
//!   translates the template before filling it.

use std::borrow::Cow;

/// Text that can be shown in the interface.
pub trait Txt<'a> {
    fn tr(self) -> Cow<'a, str>;
}

impl<'a> Txt<'a> for &'a str {
    fn tr(self) -> Cow<'a, str> {
        Cow::Borrowed(tuxrgb_core::i18n::tr(self))
    }
}

impl<'a> Txt<'a> for String {
    fn tr(self) -> Cow<'a, str> {
        Cow::Owned(self)
    }
}

impl<'a> Txt<'a> for &'a String {
    fn tr(self) -> Cow<'a, str> {
        Cow::Borrowed(self.as_str())
    }
}

impl<'a> Txt<'a> for Cow<'a, str> {
    fn tr(self) -> Cow<'a, str> {
        self
    }
}

/// Translates a French source string.
#[inline]
pub fn t(s: &str) -> &str {
    tuxrgb_core::i18n::tr(s)
}

/// `format!` for translated messages: positional `{}` only.
///
/// ```ignore
/// trf!("{} LED sur {}", n, name)
/// ```
macro_rules! trf {
    ($template:expr $(, $arg:expr)* $(,)?) => {
        tuxrgb_core::i18n::fill(
            tuxrgb_core::i18n::tr($template),
            &[$(&$arg as &dyn std::fmt::Display),*],
        )
    };
}
#[allow(unused_imports)]
pub(crate) use trf;
