use super::{Element, UiStyle, element};
use crate::ui::css::{Background, Color, Length, TextAlign};
use serde::Deserialize;

#[derive(Clone, Debug, Default, UiStyle)]
pub struct ButtonStyle {
    pub background: Option<Background>,
    pub foreground: Option<Color>,
    pub font_size: Option<Length>,
    pub text_align: Option<TextAlign>,
    pub left: Option<Length>,
    pub right: Option<Length>,
    pub top: Option<Length>,
    pub bottom: Option<Length>,
}

#[element(style = ButtonStyle)]
#[derive(Clone, Debug, Default, Deserialize, Element)]
#[serde(deny_unknown_fields)]
pub struct Button {
    #[ui(text)]
    #[serde(default, rename = "$text")]
    pub text: String,
}
