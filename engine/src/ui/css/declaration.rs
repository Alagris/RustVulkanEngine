use anyhow::{Result, anyhow, bail};
use cssparser::{Parser, ParserInput, Token};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Pixels(f32),
    Percent(f32),
}

impl FromStr for Length {
    type Err = anyhow::Error;
    fn from_str(source: &str) -> Result<Self> {
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let value = match parser.next().map_err(|error| anyhow!("{error:?}"))? {
            Token::Dimension { value, unit, .. }
                if unit.eq_ignore_ascii_case("px") && value.is_finite() =>
            {
                Self::Pixels(*value)
            }
            Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
                Self::Percent(*unit_value * 100.0)
            }
            _ => bail!("expected a finite pixel or percentage value"),
        };
        parser
            .expect_exhausted()
            .map_err(|error| anyhow!("{error:?}"))?;
        if matches!(value, Self::Percent(value) if !value.is_finite()) {
            bail!("expected a finite pixel or percentage value");
        }
        Ok(value)
    }
}
impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pixels(value) => write!(f, "{value}px"),
            Self::Percent(value) => write!(f, "{value}%"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl FromStr for Color {
    type Err = anyhow::Error;
    fn from_str(source: &str) -> Result<Self> {
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let (red, green, blue, alpha) = match parser.next().map_err(|error| anyhow!("{error:?}"))? {
            Token::Ident(name) if name.eq_ignore_ascii_case("transparent") => (0, 0, 0, 0.0),
            Token::Ident(name) => {
                let (r, g, b) = cssparser::color::parse_named_color(name)
                    .map_err(|_| anyhow!("unknown color '{name}'"))?;
                (r, g, b, 1.0)
            }
            Token::Hash(value) | Token::IDHash(value) => {
                cssparser::color::parse_hash_color(value.as_bytes())
                    .map_err(|_| anyhow!("invalid hex color"))?
            }
            _ => bail!("expected a named or hexadecimal color"),
        };
        parser
            .expect_exhausted()
            .map_err(|error| anyhow!("{error:?}"))?;
        Ok(Self {
            red,
            green,
            blue,
            alpha: (alpha * 255.0).round() as u8,
        })
    }
}
impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#{:02x}{:02x}{:02x}{:02x}",
            self.red, self.green, self.blue, self.alpha
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Background {
    Color(Color),
    Image(String),
}
impl FromStr for Background {
    type Err = anyhow::Error;
    fn from_str(source: &str) -> Result<Self> {
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let token = parser.next().map_err(|error| anyhow!("{error:?}"))?.clone();
        let url = match token {
            Token::UnquotedUrl(url) => Some(url.to_string()),
            Token::Function(name) if name.eq_ignore_ascii_case("url") => Some(
                parser
                    .parse_nested_block(|input| {
                        let url = input.expect_string()?.to_string();
                        input.expect_exhausted()?;
                        Ok::<_, cssparser::ParseError<'_, ()>>(url)
                    })
                    .map_err(|error| anyhow!("{error:?}"))?,
            ),
            _ => None,
        };
        if let Some(url) = url {
            parser
                .expect_exhausted()
                .map_err(|error| anyhow!("{error:?}"))?;
            if url.is_empty() {
                bail!("background image URL must not be empty");
            }
            Ok(Self::Image(url))
        } else {
            Ok(Self::Color(source.parse()?))
        }
    }
}
impl fmt::Display for Background {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Color(color) => color.fmt(f),
            Self::Image(url) => {
                f.write_str("url(")?;
                cssparser::serialize_string(url, f)?;
                f.write_str(")")
            }
        }
    }
}

macro_rules! string_serde {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer)?
                    .parse()
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}
string_serde!(Length);
string_serde!(Color);
string_serde!(Background);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Declaration {
    Background(Background),
    Foreground(Color),
    TextAlign(TextAlign),
    FontSize(Length),
    Left(Length),
    Right(Length),
    Top(Length),
    Bottom(Length),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Property {
    Background,
    Foreground,
    TextAlign,
    FontSize,
    Left,
    Right,
    Top,
    Bottom,
}

impl Declaration {
    pub(crate) fn parse(name: &str, value: &str) -> Result<Self> {
        let length = || -> Result<Length> { value.parse() };
        Ok(match name {
            "background" => Self::Background(value.parse()?),
            "foreground" => Self::Foreground(value.parse()?),
            "text-align" => Self::TextAlign(match value.to_ascii_lowercase().as_str() {
                "left" => TextAlign::Left,
                "center" => TextAlign::Center,
                "right" => TextAlign::Right,
                _ => bail!("expected left, center or right"),
            }),
            "font-size" => {
                let value = length()?;
                if matches!(value, Length::Pixels(v) | Length::Percent(v) if v < 0.0) {
                    bail!("font size must be non-negative");
                }
                Self::FontSize(value)
            }
            "left" => Self::Left(length()?),
            "right" => Self::Right(length()?),
            "top" => Self::Top(length()?),
            "bottom" => Self::Bottom(length()?),
            _ => bail!("unsupported style property '{name}'"),
        })
    }
    pub(crate) fn property(&self) -> Property {
        match self {
            Self::Background(_) => Property::Background,
            Self::Foreground(_) => Property::Foreground,
            Self::TextAlign(_) => Property::TextAlign,
            Self::FontSize(_) => Property::FontSize,
            Self::Left(_) => Property::Left,
            Self::Right(_) => Property::Right,
            Self::Top(_) => Property::Top,
            Self::Bottom(_) => Property::Bottom,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Style {
    pub background: Option<Background>,
    pub foreground: Option<Color>,
    pub text_align: Option<TextAlign>,
    pub font_size: Option<Length>,
    pub left: Option<Length>,
    pub right: Option<Length>,
    pub top: Option<Length>,
    pub bottom: Option<Length>,
}
impl Style {
    pub fn parse_inline(source: &str) -> Result<Self> {
        let mut style = Self::default();
        for declaration in super::Stylesheet::parse_inline(source)? {
            style.set(declaration.property(), Some(declaration));
        }
        Ok(style)
    }

    pub fn to_inline_string(&self) -> String {
        let mut declarations = Vec::new();
        for property in PROPERTIES {
            if let Some(declaration) = self.get(property) {
                declarations.push(match declaration {
                    Declaration::Background(value) => format!("background: {value};"),
                    Declaration::Foreground(value) => format!("foreground: {value};"),
                    Declaration::TextAlign(value) => format!(
                        "text-align: {};",
                        match value {
                            TextAlign::Left => "left",
                            TextAlign::Center => "center",
                            TextAlign::Right => "right",
                        }
                    ),
                    Declaration::FontSize(value) => format!("font-size: {value};"),
                    Declaration::Left(value) => format!("left: {value};"),
                    Declaration::Right(value) => format!("right: {value};"),
                    Declaration::Top(value) => format!("top: {value};"),
                    Declaration::Bottom(value) => format!("bottom: {value};"),
                });
            }
        }
        declarations.join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
    pub(crate) fn get(&self, property: Property) -> Option<Declaration> {
        match property {
            Property::Background => self.background.clone().map(Declaration::Background),
            Property::Foreground => self.foreground.map(Declaration::Foreground),
            Property::TextAlign => self.text_align.map(Declaration::TextAlign),
            Property::FontSize => self.font_size.map(Declaration::FontSize),
            Property::Left => self.left.map(Declaration::Left),
            Property::Right => self.right.map(Declaration::Right),
            Property::Top => self.top.map(Declaration::Top),
            Property::Bottom => self.bottom.map(Declaration::Bottom),
        }
    }
    pub(crate) fn set(&mut self, property: Property, value: Option<Declaration>) {
        match (property, value) {
            (Property::Background, Some(Declaration::Background(v))) => self.background = Some(v),
            (Property::Foreground, Some(Declaration::Foreground(v))) => self.foreground = Some(v),
            (Property::TextAlign, Some(Declaration::TextAlign(v))) => self.text_align = Some(v),
            (Property::FontSize, Some(Declaration::FontSize(v))) => self.font_size = Some(v),
            (Property::Left, Some(Declaration::Left(v))) => self.left = Some(v),
            (Property::Right, Some(Declaration::Right(v))) => self.right = Some(v),
            (Property::Top, Some(Declaration::Top(v))) => self.top = Some(v),
            (Property::Bottom, Some(Declaration::Bottom(v))) => self.bottom = Some(v),
            (Property::Background, None) => self.background = None,
            (Property::Foreground, None) => self.foreground = None,
            (Property::TextAlign, None) => self.text_align = None,
            (Property::FontSize, None) => self.font_size = None,
            (Property::Left, None) => self.left = None,
            (Property::Right, None) => self.right = None,
            (Property::Top, None) => self.top = None,
            (Property::Bottom, None) => self.bottom = None,
            _ => unreachable!("declaration and property must match"),
        }
    }
}

pub(crate) const PROPERTIES: [Property; 8] = [
    Property::Background,
    Property::Foreground,
    Property::TextAlign,
    Property::FontSize,
    Property::Left,
    Property::Right,
    Property::Top,
    Property::Bottom,
];
