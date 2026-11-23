use super::{Attributes, Direction, UiStyle, deserialize_style_attribute};
use crate::ui::css::Style;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Attributes)]
#[serde(bound(deserialize = "S: UiStyle"))]
pub struct ElementData<S: UiStyle = Style> {
    pub id: Option<String>,
    pub name: Option<String>,
    pub class: Option<String>,
    #[serde(default, deserialize_with = "deserialize_style_attribute")]
    pub style: S,
}
