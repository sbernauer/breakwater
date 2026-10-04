use std::fmt::{self, Display};

use color_eyre::eyre::{self, bail};

use crate::{refactored, simd};

/// Different parser implementations, with their strengths and weaknesses.
/// Not all parsers implement the same feature set.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ParserImplementation {
    /// The OG parser, most feature rich. Ugly but performant.
    #[default]
    Original,

    /// The original parser restructured into smaller functions. Doesn't support binary-sync-pixels.
    Refactored,

    /// Experimental SIMD parser. Doesn't support the binary commands.
    Simd,
}

impl ParserImplementation {
    /// Checks that the parser supports everything this build enables, errors otherwise.
    pub fn check_supported(self) -> eyre::Result<()> {
        let unsupported_feature = match self {
            Self::Original => None,
            Self::Refactored => refactored::UNSUPPORTED_ENABLED_FEATURE,
            Self::Simd => simd::UNSUPPORTED_ENABLED_FEATURE,
        };
        if let Some(feature) = unsupported_feature {
            bail!("the {self} parser doesn't support the {feature} feature this build enables");
        }
        Ok(())
    }
}

impl Display for ParserImplementation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Original => "original",
            Self::Refactored => "refactored",
            Self::Simd => "simd",
        })
    }
}
