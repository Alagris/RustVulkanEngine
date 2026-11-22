use super::Attributes;
use crate::ui::css::Style;
use anyhow::{Result, bail};

pub trait Element: Attributes + std::fmt::Debug + std::any::Any {
    fn tag(&self) -> &str;
    fn data(&self) -> &dyn Attributes;
    fn inline_style(&self) -> Style;
    fn validate_style(&self, style: &Style) -> Result<()>;
    fn is_layout(&self) -> bool {
        false
    }
    fn text(&self) -> Option<&str> {
        None
    }
    fn set_text(&mut self, _: String) -> Result<()> {
        bail!("element '{}' does not support text", self.tag())
    }
}
