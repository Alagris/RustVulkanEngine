use anyhow::{Result, anyhow};
use std::borrow::Cow;

/// Conversion at the XML/selector boundary; the stored value remains typed.
pub trait AttributeValue: Sized {
    fn attribute_value(&self) -> Option<Cow<'_, str>>;
    fn parse_attribute(value: Option<&str>) -> Result<Self>;
}

impl AttributeValue for String {
    fn attribute_value(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self))
    }

    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        value
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("required attribute cannot be removed"))
    }
}

impl<T: AttributeValue> AttributeValue for Option<T> {
    fn attribute_value(&self) -> Option<Cow<'_, str>> {
        self.as_ref().and_then(AttributeValue::attribute_value)
    }

    fn parse_attribute(value: Option<&str>) -> Result<Self> {
        value
            .map(|value| T::parse_attribute(Some(value)))
            .transpose()
    }
}
