use super::document::{Document, ElementId};
use cssparser::{
    CowRcStr, ParseError, Parser as CssParser, SourceLocation, ToCss, serialize_identifier,
};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, matches_selector};
use selectors::parser::{
    NonTSPseudoClass, ParseRelative, Parser, PseudoElement, SelectorImpl, SelectorList,
    SelectorParseErrorKind,
};
use selectors::{Element, OpaqueElement};
use std::collections::BTreeSet;
use std::fmt;
use std::hash::{Hash, Hasher};

/// UI states set by the application using `StyledDocument::set_state`.
pub const PSEUDO_STATES: &[&str] = &["hover", "active", "focus", "enabled", "disabled", "checked"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct Atom(String);

impl From<&str> for Atom {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
impl AsRef<str> for Atom {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl ToCss for Atom {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        serialize_identifier(&self.0, dest)
    }
}
impl PrecomputedHash for Atom {
    fn precomputed_hash(&self) -> u32 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut hash);
        hash.finish() as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Flag(String),
    Lang(Vec<String>),
    Direction(String),
}
impl ToCss for State {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        match self {
            Self::Flag(name) => write!(dest, ":{name}"),
            Self::Lang(languages) => write!(dest, ":lang({})", languages.join(", ")),
            Self::Direction(direction) => write!(dest, ":dir({direction})"),
        }
    }
}
impl NonTSPseudoClass for State {
    type Impl = UiSelector;
    fn is_active_or_hover(&self) -> bool {
        matches!(self, Self::Flag(name) if name == "active" || name == "hover")
    }
    fn is_user_action_state(&self) -> bool {
        self.is_active_or_hover() || matches!(self, Self::Flag(name) if name == "focus")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pseudo {}
impl ToCss for Pseudo {
    fn to_css<W: fmt::Write>(&self, _: &mut W) -> fmt::Result {
        match *self {}
    }
}
impl PseudoElement for Pseudo {
    type Impl = UiSelector;
    fn is_before_or_after(&self) -> bool {
        match *self {}
    }
    fn accepts_state_pseudo_classes(&self) -> bool {
        match *self {}
    }
}

#[derive(Clone, Debug, PartialEq)]
struct UiSelector;
impl SelectorImpl for UiSelector {
    type ExtraMatchingData<'a> = ();
    type AttrValue = Atom;
    type Identifier = Atom;
    type LocalName = Atom;
    type NamespaceUrl = Atom;
    type NamespacePrefix = Atom;
    type BorrowedLocalName = Atom;
    type BorrowedNamespaceUrl = Atom;
    type NonTSPseudoClass = State;
    type PseudoElement = Pseudo;
}

struct UiParser;
impl<'i> Parser<'i> for UiParser {
    type Impl = UiSelector;
    type Error = SelectorParseErrorKind<'i>;
    fn parse_nth_child_of(&self) -> bool {
        true
    }
    fn parse_is_and_where(&self) -> bool {
        true
    }
    fn parse_has(&self) -> bool {
        true
    }
    fn allow_forgiving_selectors(&self) -> bool {
        false
    }
    fn parse_non_ts_pseudo_class(
        &self,
        location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<State, ParseError<'i, Self::Error>> {
        if !PSEUDO_STATES
            .iter()
            .any(|state| name.eq_ignore_ascii_case(state))
        {
            return Err(location.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
            ));
        }
        Ok(State::Flag(name.to_ascii_lowercase()))
    }
    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: CowRcStr<'i>,
        input: &mut CssParser<'i, 't>,
        _: bool,
    ) -> Result<State, ParseError<'i, Self::Error>> {
        if name.eq_ignore_ascii_case("lang") {
            let languages = input.parse_comma_separated(|input| match input.next()?.clone() {
                cssparser::Token::Ident(value) | cssparser::Token::QuotedString(value) => {
                    Ok(value.to_ascii_lowercase())
                }
                cssparser::Token::Delim('*') => Ok("*".to_owned()),
                token => {
                    Err(input.new_error(cssparser::BasicParseErrorKind::UnexpectedToken(token)))
                }
            })?;
            return Ok(State::Lang(languages));
        }
        if name.eq_ignore_ascii_case("dir") {
            let direction = input.expect_ident()?.to_ascii_lowercase();
            if direction != "ltr" && direction != "rtl" {
                return Err(input.new_custom_error(
                    SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
                ));
            }
            return Ok(State::Direction(direction));
        }
        Err(
            input.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            )),
        )
    }
    fn parse_pseudo_element(
        &self,
        location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<Pseudo, ParseError<'i, Self::Error>> {
        Err(
            location.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            )),
        )
    }
    fn parse_functional_pseudo_element<'t>(
        &self,
        name: CowRcStr<'i>,
        input: &mut CssParser<'i, 't>,
    ) -> Result<Pseudo, ParseError<'i, Self::Error>> {
        Err(
            input.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            )),
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Selector(SelectorList<UiSelector>);

impl Selector {
    pub(crate) fn parse<'i, 't>(
        input: &mut CssParser<'i, 't>,
    ) -> Result<Self, ParseError<'i, SelectorParseErrorKind<'i>>> {
        SelectorList::parse(&UiParser, input, ParseRelative::No).map(Self)
    }

    pub(crate) fn matches(
        &self,
        document: &Document,
        id: ElementId,
        states: &[BTreeSet<String>],
    ) -> bool {
        let element = MatchElement {
            document,
            id,
            states,
        };
        self.0.slice().iter().any(|selector| {
            let mut caches = SelectorCaches::default();
            let mut context = MatchingContext::new(
                MatchingMode::Normal,
                None,
                &mut caches,
                QuirksMode::NoQuirks,
                NeedsSelectorFlags::No,
                MatchingForInvalidation::No,
            );
            matches_selector(selector, 0, None, &element, &mut context)
        })
    }
}

#[derive(Clone, Debug)]
struct MatchElement<'a> {
    document: &'a Document,
    id: ElementId,
    states: &'a [BTreeSet<String>],
}

impl MatchElement<'_> {
    fn with_id(&self, id: ElementId) -> Self {
        Self { id, ..self.clone() }
    }
    fn sibling(&self, next: bool) -> Option<Self> {
        let parent = self.document.parent(self.id)?;
        let siblings = self.document.children(parent)?;
        let index = siblings.iter().position(|id| *id == self.id)?;
        let index = if next {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        Some(self.with_id(*siblings.get(index)?))
    }
}

impl Element for MatchElement<'_> {
    type Impl = UiSelector;
    fn opaque(&self) -> OpaqueElement {
        let node = self.document.node(self.id).expect("live selector element");
        OpaqueElement::new(&node.identity)
    }
    fn parent_element(&self) -> Option<Self> {
        self.document.parent(self.id).map(|id| self.with_id(id))
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn pseudo_element_originating_element(&self) -> Option<Self> {
        None
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling(false)
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling(true)
    }
    fn first_element_child(&self) -> Option<Self> {
        self.document
            .children(self.id)?
            .first()
            .map(|id| self.with_id(*id))
    }
    fn is_html_element_in_html_document(&self) -> bool {
        false
    }
    fn has_local_name(&self, name: &Atom) -> bool {
        self.document
            .element(self.id)
            .expect("live selector element")
            .tag()
            == name.0
    }
    fn has_namespace(&self, ns: &Atom) -> bool {
        ns.0.is_empty()
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.document.element(self.id).unwrap().tag()
            == other.document.element(other.id).unwrap().tag()
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&Atom>,
        name: &Atom,
        operation: &AttrSelectorOperation<&Atom>,
    ) -> bool {
        if matches!(ns, NamespaceConstraint::Specific(url) if !url.0.is_empty()) {
            return false;
        }
        self.document
            .element(self.id)
            .unwrap()
            .attribute(&name.0)
            .is_some_and(|value| operation.eval_str(&value))
    }
    fn match_non_ts_pseudo_class(&self, pc: &State, _: &mut MatchingContext<UiSelector>) -> bool {
        match pc {
            State::Flag(name) => self
                .states
                .get(self.id.0)
                .is_some_and(|states| states.contains(name)),
            State::Lang(languages) => {
                let mut current = Some(self.with_id(self.id));
                while let Some(element) = current {
                    let data = element.document.element(element.id).unwrap().data();
                    if let Some(lang) = data
                        .attribute("xml:lang")
                        .or_else(|| data.attribute("lang"))
                    {
                        let lang = lang.to_ascii_lowercase();
                        return languages
                            .iter()
                            .any(|expected| language_matches(expected, &lang));
                    }

                    current = element.parent_element();
                }
                false
            }
            State::Direction(direction) => {
                let mut current = Some(self.with_id(self.id));
                while let Some(element) = current {
                    if let Some(value) = element
                        .document
                        .element(element.id)
                        .unwrap()
                        .data()
                        .attribute("dir")
                    {
                        return value.eq_ignore_ascii_case(direction);
                    }
                    current = element.parent_element();
                }
                direction == "ltr"
            }
        }
    }
    fn match_pseudo_element(&self, pseudo: &Pseudo, _: &mut MatchingContext<UiSelector>) -> bool {
        match *pseudo {}
    }
    fn apply_selector_flags(&self, _: ElementSelectorFlags) {}
    fn is_link(&self) -> bool {
        false
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }
    fn has_id(&self, id: &Atom, case: CaseSensitivity) -> bool {
        self.document
            .element(self.id)
            .unwrap()
            .data()
            .attribute("id")
            .is_some_and(|value| case.eq(value.as_bytes(), id.0.as_bytes()))
    }
    fn has_class(&self, class: &Atom, case: CaseSensitivity) -> bool {
        self.document
            .element(self.id)
            .unwrap()
            .data()
            .attribute("class")
            .is_some_and(|value| {
                super::document::class_names(&value)
                    .any(|part| case.eq(part.as_bytes(), class.0.as_bytes()))
            })
    }
    fn has_custom_state(&self, state: &Atom) -> bool {
        self.states
            .get(self.id.0)
            .is_some_and(|states| states.contains(&state.0))
    }
    fn imported_part(&self, _: &Atom) -> Option<Atom> {
        None
    }
    fn is_part(&self, _: &Atom) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.document.children(self.id).unwrap().is_empty()
            && self
                .document
                .element(self.id)
                .unwrap()
                .text()
                .is_none_or(str::is_empty)
    }
    fn is_root(&self) -> bool {
        self.id == self.document.root()
    }
    fn add_element_unique_hashes(&self, _: &mut BloomFilter) -> bool {
        false
    }
}

fn language_matches(range: &str, language: &str) -> bool {
    if language.is_empty() {
        return false;
    }
    let mut language = language.split('-');
    let mut range = range.split('-');
    let Some(first) = range.next() else {
        return false;
    };
    let Some(language_first) = language.next() else {
        return false;
    };
    if first != "*" && first != language_first {
        return false;
    }
    for subtag in range {
        if subtag == "*" {
            continue;
        }
        let mut found = false;
        for value in language.by_ref() {
            if value == subtag {
                found = true;
                break;
            }
            if value.len() == 1 {
                return false;
            }
        }
        if !found {
            return false;
        }
    }
    true
}
