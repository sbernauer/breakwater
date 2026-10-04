/// The parsers that can be selected, e.g. by the server per connection
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ParserKind {
    /// The proven parser
    #[default]
    Original,
}
