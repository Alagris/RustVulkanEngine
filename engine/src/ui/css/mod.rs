//! Parsed UI stylesheets and typed evaluated styles.

mod declaration;
mod rule;
mod styled_document;
mod stylesheet;

pub use declaration::{Background, Color, Declaration, Length, Style, TextAlign};
pub use rule::Rule;
pub use styled_document::StyledDocument;
pub use stylesheet::Stylesheet;
