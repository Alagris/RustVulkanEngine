use super::{AttributeValue, Element, UiStyle, element};
use crate::ui::css::{Background, Color, Length, TextAlign};
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StackOrientation {
    Horizontal,
    #[default]
    Vertical,
}

impl AttributeValue for StackOrientation {
    fn attribute_value(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }))
    }
    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        match value {
            Some("horizontal") => Ok(Self::Horizontal),
            Some("vertical") => Ok(Self::Vertical),
            _ => bail!("invalid Stack orientation; expected horizontal or vertical"),
        }
    }
}

impl<'de> Deserialize<'de> for StackOrientation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse_attribute(Some(&String::deserialize(deserializer)?))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, UiStyle)]
pub struct StackStyle {
    pub background: Option<Background>,
    pub foreground: Option<Color>,
    pub font_size: Option<Length>,
    pub text_align: Option<TextAlign>,
    pub left: Option<Length>,
    pub right: Option<Length>,
    pub top: Option<Length>,
    pub bottom: Option<Length>,
}

#[element(style = StackStyle, layout)]
#[derive(Clone, Debug, Default, Deserialize, Element)]
#[serde(deny_unknown_fields)]
pub struct Stack {
    pub orientation: Option<StackOrientation>,
}

impl Stack {
    pub fn orientation(&self) -> StackOrientation {
        self.orientation.unwrap_or_default()
    }
    pub fn set_orientation(&mut self, orientation: StackOrientation) {
        self.orientation = Some(orientation);
    }
}
