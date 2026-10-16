use thiserror::Error;

/// The text given to a term's `from_str` names no such term.
#[derive(Debug, Error)]
#[error("unknown {what} `{text}` (expected {expected})")]
pub struct UnknownTerm {
    what: &'static str,
    text: String,
    expected: String,
}

impl UnknownTerm {
    /// What [`vocabulary!`](crate::vocabulary) reports for text that is no
    /// variant's.
    pub fn new(what: &'static str, text: &str, expected: String) -> Self {
        Self {
            what,
            text: text.to_owned(),
            expected,
        }
    }
}

/// Gives a fieldless enum the text its variants are stored, printed and parsed
/// as: `as_str`, `ALL`, `Display` and `FromStr` (failing with [`UnknownTerm`]).
#[macro_export]
macro_rules! vocabulary {
    ($ty:ident, $what:literal { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The text the variant is stored, printed and parsed as.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::std::str::FromStr for $ty {
            type Err = $crate::UnknownTerm;

            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| v.as_str() == s)
                    .ok_or_else(|| {
                        let expected: Vec<_> = Self::ALL.iter().map(|v| v.as_str()).collect();
                        $crate::UnknownTerm::new($what, s, expected.join(", "))
                    })
            }
        }
    };
}
