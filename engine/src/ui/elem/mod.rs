//! Typed UI elements and their common attributes.

mod attribute_value;
mod attributes;
mod button;
mod canvas;
mod direction;
mod element;
mod element_data;
mod grid;
mod image;
mod label;
mod layout;
mod list;
mod overlay;
mod scroll_pane;
mod stack;
mod table;
mod ui_style;

pub use anyhow;
pub use attribute_value::AttributeValue;
pub use attributes::Attributes;
pub use button::{Button, ButtonStyle};
pub use canvas::{Canvas, CanvasStyle};
pub use component_macros::{Attributes, Element, UiStyle, element};
pub use direction::Direction;
pub use element::Element;
pub use element_data::ElementData;
pub use grid::{Grid, GridStyle};
pub use image::{Image, ImageStyle};
pub use label::{Label, LabelStyle};
pub use layout::Layout;
pub use list::{List, ListStyle};
pub use overlay::{Overlay, OverlayStyle};
pub use scroll_pane::{ScrollPane, ScrollPaneStyle};
pub use stack::{Stack, StackOrientation, StackStyle};
pub use table::{Table, TableStyle};
pub use ui_style::{
    UiStyle, deserialize_style_attribute, parse_style_attribute, style_attribute, validate_style,
};
