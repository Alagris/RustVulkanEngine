use super::Declaration;
use crate::ui::selector::Selector;

#[derive(Clone, Debug)]
pub struct Rule {
    pub(crate) selector: Selector,
    pub(crate) source: String,
    pub(crate) declarations: Vec<Declaration>,
}

impl Rule {
    pub fn selector_source(&self) -> &str {
        &self.source
    }

    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        self.source == other.source && self.declarations == other.declarations
    }
}
