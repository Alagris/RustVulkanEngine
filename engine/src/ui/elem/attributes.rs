use anyhow::Result;
use std::borrow::Cow;

/// Object-safe string adapter for typed fields, used by XML editing and selectors.
pub trait Attributes {
    fn attribute(&self, name: &str) -> Option<Cow<'_, str>>;
    fn set_attribute(&mut self, name: &str, value: Option<&str>) -> Result<()>;
}
