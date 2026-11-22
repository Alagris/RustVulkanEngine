use super::declaration::Declaration;
use super::rule::Rule;
use crate::ui::selector::Selector;
use anyhow::{Context, Result, anyhow};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
};
use selectors::parser::SelectorParseErrorKind;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
struct RawRule {
    selector: Selector,
    source: String,
    declarations: Vec<(String, String)>,
}

/// A parsed, ordered list of UI style rules.
#[derive(Clone, Debug)]
pub struct Stylesheet {
    pub(crate) rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub(crate) fn parse_inline(source: &str) -> Result<Vec<Declaration>> {
        validate_delimiters(source).context("invalid inline style")?;
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let mut declarations = Declarations;
        RuleBodyParser::new(&mut parser, &mut declarations)
            .map(|item| {
                let (name, value) = item.map_err(|(error, source)| {
                    anyhow!("invalid inline style near {source:?}: {error:?}")
                })?;
                Declaration::parse(&name, &value)
                    .with_context(|| format!("invalid value '{value}' for '{name}'"))
            })
            .collect()
    }

    pub fn parse(source: &str) -> Result<Self> {
        validate_delimiters(source)?;
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let mut rule_parser = SheetParser;
        let mut rules = Vec::new();
        for item in StyleSheetParser::new(&mut parser, &mut rule_parser) {
            let rule = item.map_err(|(error, source)| {
                anyhow!("invalid stylesheet near {source:?}: {error:?}")
            })?;
            let declarations = rule
                .declarations
                .into_iter()
                .map(|(name, value)| {
                    Declaration::parse(&name, &value)
                        .with_context(|| format!("invalid value '{value}' for '{name}'"))
                })
                .collect::<Result<Vec<_>>>()?;
            rules.push(Rule {
                selector: rule.selector,
                source: rule.source,
                declarations,
            });
        }
        Ok(Self { rules })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read stylesheet {}", path.display()))?;
        Self::parse(&source).with_context(|| format!("invalid stylesheet {}", path.display()))
    }
}

fn validate_delimiters(source: &str) -> Result<()> {
    fn closed(raw: &str, delimiter: u8) -> bool {
        let bytes = raw.as_bytes();
        if bytes.last() != Some(&delimiter) {
            return false;
        }
        let escapes = bytes[..bytes.len() - 1]
            .iter()
            .rev()
            .take_while(|byte| **byte == b'\\')
            .count();
        escapes % 2 == 0
    }

    fn check<'i>(
        parser: &mut Parser<'i, '_>,
        source: &'i str,
        closing: Option<u8>,
    ) -> Result<(), ParseError<'i, &'static str>> {
        loop {
            let start = parser.position().byte_index();
            let token = match parser.next_including_whitespace_and_comments() {
                Ok(token) => token.clone(),
                Err(_) => {
                    if closing.is_some_and(|expected| {
                        source.as_bytes().get(parser.position().byte_index()) != Some(&expected)
                    }) {
                        return Err(parser.new_custom_error("unclosed stylesheet block"));
                    }
                    return Ok(());
                }
            };
            let end = parser.position().byte_index();
            let raw = &source[start..end];
            match token {
                cssparser::Token::Function(_) | cssparser::Token::ParenthesisBlock => {
                    parser.parse_nested_block(|nested| check(nested, source, Some(b')')))?;
                }
                cssparser::Token::SquareBracketBlock => {
                    parser.parse_nested_block(|nested| check(nested, source, Some(b']')))?;
                }
                cssparser::Token::CurlyBracketBlock => {
                    parser.parse_nested_block(|nested| check(nested, source, Some(b'}')))?;
                }
                cssparser::Token::Comment(_) if raw.len() < 4 || !raw.ends_with("*/") => {
                    return Err(parser.new_custom_error("unterminated stylesheet comment"));
                }
                cssparser::Token::QuotedString(_)
                    if raw.len() < 2 || !closed(raw, raw.as_bytes()[0]) =>
                {
                    return Err(parser.new_custom_error("unterminated stylesheet string"));
                }
                cssparser::Token::UnquotedUrl(_) if !closed(raw, b')') => {
                    return Err(parser.new_custom_error("unterminated stylesheet URL"));
                }
                cssparser::Token::BadString(_)
                | cssparser::Token::BadUrl(_)
                | cssparser::Token::CloseParenthesis
                | cssparser::Token::CloseSquareBracket
                | cssparser::Token::CloseCurlyBracket => {
                    return Err(parser.new_custom_error("invalid stylesheet token"));
                }
                _ => {}
            }
        }
    }
    let mut input = ParserInput::new(source);
    check(&mut Parser::new(&mut input), source, None)
        .map_err(|error| anyhow!("invalid stylesheet syntax: {error:?}"))
}

struct SheetParser;
impl<'i> AtRuleParser<'i> for SheetParser {
    type Prelude = ();
    type AtRule = RawRule;
    type Error = SelectorParseErrorKind<'i>;
}
impl<'i> QualifiedRuleParser<'i> for SheetParser {
    type Prelude = (Selector, String);
    type QualifiedRule = RawRule;
    type Error = SelectorParseErrorKind<'i>;
    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        let selector = Selector::parse(input)?;
        Ok((selector, input.slice_from(start).trim().to_owned()))
    }
    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<RawRule, ParseError<'i, Self::Error>> {
        let mut parser = Declarations;
        let mut declarations = Vec::new();
        for declaration in RuleBodyParser::new(input, &mut parser) {
            declarations.push(declaration.map_err(|(error, _)| error)?);
        }
        Ok(RawRule {
            selector: prelude.0,
            source: prelude.1,
            declarations,
        })
    }
}

struct Declarations;
impl<'i> DeclarationParser<'i> for Declarations {
    type Declaration = (String, String);
    type Error = SelectorParseErrorKind<'i>;
    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        _: &ParserState,
    ) -> Result<Self::Declaration, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next_including_whitespace_and_comments().is_ok() {}
        let value = remove_comments(input.slice_from(start))?.trim().to_owned();
        if value.is_empty() {
            return Err(input.new_error(cssparser::BasicParseErrorKind::EndOfInput));
        }

        Ok((name.to_ascii_lowercase(), value))
    }
}

fn remove_comments<'i>(
    source: &'i str,
) -> Result<String, ParseError<'i, SelectorParseErrorKind<'i>>> {
    fn collect<'i>(
        input: &mut Parser<'i, '_>,
        ranges: &mut Vec<(usize, usize)>,
    ) -> Result<(), ParseError<'i, SelectorParseErrorKind<'i>>> {
        loop {
            let start = input.position().byte_index();
            let token = match input.next_including_whitespace_and_comments() {
                Ok(token) => token.clone(),
                Err(_) => return Ok(()),
            };
            match token {
                cssparser::Token::Comment(_) => ranges.push((start, input.position().byte_index())),
                cssparser::Token::Function(_)
                | cssparser::Token::ParenthesisBlock
                | cssparser::Token::SquareBracketBlock
                | cssparser::Token::CurlyBracketBlock => {
                    input.parse_nested_block(|nested| collect(nested, ranges))?;
                }
                _ => {}
            }
        }
    }
    let mut input = ParserInput::new(source);
    let mut ranges = Vec::new();
    collect(&mut Parser::new(&mut input), &mut ranges)?;
    let mut output = String::new();
    let mut previous = 0;
    for (start, end) in ranges {
        output.push_str(&source[previous..start]);
        output.push(' ');
        previous = end;
    }
    output.push_str(&source[previous..]);
    Ok(output)
}

impl<'i> AtRuleParser<'i> for Declarations {
    type Prelude = ();
    type AtRule = (String, String);
    type Error = SelectorParseErrorKind<'i>;
}
impl<'i> QualifiedRuleParser<'i> for Declarations {
    type Prelude = ();
    type QualifiedRule = (String, String);
    type Error = SelectorParseErrorKind<'i>;
}
impl<'i> RuleBodyItemParser<'i, (String, String), SelectorParseErrorKind<'i>> for Declarations {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}
