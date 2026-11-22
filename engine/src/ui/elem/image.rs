use super::{Element, UiStyle, element};
use crate::ui::css::{Background, Color, Length};
use serde::Deserialize;

#[derive(Clone, Debug, Default, UiStyle)]
pub struct ImageStyle {
    pub background: Option<Background>,
    pub foreground: Option<Color>,
    pub left: Option<Length>,
    pub right: Option<Length>,
    pub top: Option<Length>,
    pub bottom: Option<Length>,
}

#[element(style = ImageStyle)]
#[derive(Clone, Debug, Default, Deserialize, Element)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub src: Option<String>,
}
