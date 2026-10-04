use color_eyre::eyre::{self, bail};

use crate::fear;

/// The parsers that can be selected, e.g. by the server per connection
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ParserKind {
    /// The proven parser
    #[default]
    Original,

    /// Experimental SIMD parser. Doesn't support the binary commands.
    Fear,
}

impl ParserKind {
    /// Checks that the parser supports everything this build enables.
    ///
    /// # Errors
    ///
    /// If the parser doesn't support an enabled feature.
    pub fn check_supported(self) -> eyre::Result<()> {
        match self {
            Self::Original => Ok(()),
            Self::Fear => match fear::UNSUPPORTED_ENABLED_FEATURE {
                Some(feature) => {
                    bail!(
                        "the fear parser doesn't support the {feature} feature this build enables"
                    )
                }
                None => Ok(()),
            },
        }
    }
}
