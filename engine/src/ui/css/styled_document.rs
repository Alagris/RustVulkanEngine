use super::Stylesheet;
use super::declaration::{Declaration, PROPERTIES, Property, Style};
use crate::ui::document::{Document, ElementId};
use crate::ui::selector::PSEUDO_STATES;
use anyhow::{Result, anyhow, bail};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
struct Computed {
    base: Style,
    overrides: BTreeMap<Property, Option<Declaration>>,
}

impl Computed {
    fn resolve(&self) -> Style {
        let mut style = self.base.clone();
        for (property, value) in &self.overrides {
            style.set(*property, value.clone());
        }
        style
    }
}

/// A document with ordered stylesheets and sparse current pseudo-state overrides.
#[derive(Debug)]
pub struct StyledDocument {
    document: Document,
    sheets: Vec<Stylesheet>,
    states: Vec<BTreeSet<String>>,
    styles: Vec<Option<Computed>>,
    dirty: bool,
}

impl StyledDocument {
    pub fn new(document: Document, sheets: Vec<Stylesheet>) -> Self {
        let mut result = Self {
            document,
            sheets,
            states: Vec::new(),
            styles: Vec::new(),
            dirty: true,
        };
        result.refresh();
        result
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Editing the document invalidates structural selector matches.
    pub fn document_mut(&mut self) -> &mut Document {
        self.dirty = true;
        &mut self.document
    }

    pub fn set_state(&mut self, id: ElementId, state: &str, active: bool) -> Result<()> {
        if self.document.element(id).is_none() {
            bail!("element does not exist");
        }
        if !PSEUDO_STATES
            .iter()
            .any(|known| state.eq_ignore_ascii_case(known))
        {
            bail!("unsupported pseudo-state '{state}'");
        }
        self.ensure_size();
        let changed = if active {
            self.states[id.0].insert(state.to_ascii_lowercase())
        } else {
            self.states[id.0].remove(&state.to_ascii_lowercase())
        };
        if changed {
            self.dirty = true;
        }
        Ok(())
    }

    pub fn style(&mut self, id: ElementId) -> Result<Style> {
        self.refresh();
        let style = self
            .styles
            .get(id.0)
            .and_then(Option::as_ref)
            .map(Computed::resolve)
            .ok_or_else(|| anyhow!("element does not exist"))?;
        self.document
            .element(id)
            .expect("live styled element")
            .validate_style(&style)?;
        Ok(style)
    }

    pub fn override_count(&mut self, id: ElementId) -> Result<usize> {
        self.refresh();
        self.styles
            .get(id.0)
            .and_then(Option::as_ref)
            .map(|style| style.overrides.len())
            .ok_or_else(|| anyhow!("element does not exist"))
    }

    /// Replace the ordered sheets, recomputing only elements matched by changed rules.
    pub fn set_stylesheets(&mut self, sheets: Vec<Stylesheet>) -> Vec<ElementId> {
        self.refresh();
        let old_rules = self
            .sheets
            .iter()
            .flat_map(|sheet| &sheet.rules)
            .collect::<Vec<_>>();
        let new_rules = sheets
            .iter()
            .flat_map(|sheet| &sheet.rules)
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for index in 0..old_rules.len().max(new_rules.len()) {
            match (old_rules.get(index), new_rules.get(index)) {
                (Some(old), Some(new)) if old.equivalent(new) => {}
                (old, new) => {
                    if let Some(old) = old {
                        changed.push(*old);
                    }
                    if let Some(new) = new {
                        changed.push(*new);
                    }
                }
            }
        }
        let empty_states = vec![BTreeSet::new(); self.document.nodes.len()];
        let affected = self
            .document
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                node.as_ref()?;
                let id = ElementId(index);
                changed
                    .iter()
                    .any(|rule| {
                        rule.selector.matches(&self.document, id, &self.states)
                            || rule.selector.matches(&self.document, id, &empty_states)
                    })
                    .then_some(id)
            })
            .collect::<Vec<_>>();
        self.sheets = sheets;
        for id in &affected {
            self.recompute(*id, &empty_states);
        }
        affected
    }

    fn ensure_size(&mut self) {
        self.states
            .resize_with(self.document.nodes.len(), BTreeSet::new);
        self.styles.resize_with(self.document.nodes.len(), || None);
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.ensure_size();
        let empty_states = vec![BTreeSet::new(); self.document.nodes.len()];
        for index in 0..self.document.nodes.len() {
            let id = ElementId(index);
            if self.document.node(id).is_some() {
                self.recompute(id, &empty_states);
            } else {
                self.styles[index] = None;
                self.states[index].clear();
            }
        }
        self.dirty = false;
    }

    fn recompute(&mut self, id: ElementId, empty_states: &[BTreeSet<String>]) {
        let base = evaluate(&self.document, id, empty_states, &self.sheets);
        let current = evaluate(&self.document, id, &self.states, &self.sheets);
        let mut overrides = BTreeMap::new();
        for property in PROPERTIES {
            let value = current.get(property);
            if base.get(property) != value {
                overrides.insert(property, value);
            }
        }
        self.styles[id.0] = Some(Computed { base, overrides });
    }
}

fn evaluate(
    document: &Document,
    id: ElementId,
    states: &[BTreeSet<String>],
    sheets: &[Stylesheet],
) -> Style {
    let mut style = Style::default();
    for rule in sheets.iter().flat_map(|sheet| &sheet.rules) {
        if rule.selector.matches(document, id, states) {
            for declaration in &rule.declarations {
                style.set(declaration.property(), Some(declaration.clone()));
            }
        }
    }
    let inline = document
        .element(id)
        .expect("live styled element")
        .inline_style();
    for property in PROPERTIES {
        if let Some(value) = inline.get(property) {
            style.set(property, Some(value));
        }
    }
    style
}
