use super::AttributeValue;
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Ltr,
    Rtl,
}

impl AttributeValue for Direction {
    fn attribute_value(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }))
    }
    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        match value {
            Some(value) if value.eq_ignore_ascii_case("ltr") => Ok(Self::Ltr),
            Some(value) if value.eq_ignore_ascii_case("rtl") => Ok(Self::Rtl),
            _ => bail!("invalid UI direction; expected ltr or rtl"),
        }
    }
}

impl<'de> Deserialize<'de> for Direction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse_attribute(Some(&String::deserialize(deserializer)?))
            .map_err(serde::de::Error::custom)
    }
}
