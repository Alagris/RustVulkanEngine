use super::AttributeValue;
use crate::ui::css::Style;
use anyhow::Result;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

pub trait UiStyle: AttributeValue + Clone + std::fmt::Debug + Default + 'static {
    fn to_style(&self) -> Style;
    fn from_style(style: Style) -> Result<Self>;
}

pub fn style_attribute<S: UiStyle>(style: &S) -> Option<Cow<'_, str>> {
    let style = style.to_style();
    (!style.is_empty()).then(|| Cow::Owned(style.to_inline_string()))
}

pub fn parse_style_attribute<S: UiStyle>(value: Option<&str>) -> Result<S> {
    S::from_style(Style::parse_inline(value.unwrap_or(""))?)
}

pub fn validate_style<S: UiStyle>(_: &S, style: &Style) -> Result<()> {
    S::from_style(style.clone()).map(|_| ())
}

pub fn deserialize_style_attribute<'de, D: Deserializer<'de>, S: UiStyle>(
    deserializer: D,
) -> Result<S, D::Error> {
    parse_style_attribute(Some(&String::deserialize(deserializer)?))
        .map_err(|error| serde::de::Error::custom(format!("{error:#}")))
}

impl UiStyle for Style {
    fn to_style(&self) -> Style {
        self.clone()
    }
    fn from_style(style: Style) -> Result<Self> {
        Ok(style)
    }
}

impl AttributeValue for Style {
    fn attribute_value(&self) -> Option<Cow<'_, str>> {
        style_attribute(self)
    }
    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        parse_style_attribute(value)
    }
}
