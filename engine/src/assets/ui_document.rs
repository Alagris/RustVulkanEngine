use super::asset_ref::{AssetContext, current_context};
use crate::ui::Document;
use crate::ui::document::{ReferenceResolver, Spec, load_spec};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// A compiled, immutable document template. Instances have independent element identities.
#[derive(Clone, Debug)]
pub struct UiDocument {
    spec: Spec,
    context: Arc<AssetContext>,
}

impl UiDocument {
    pub(crate) fn load(path: &Path, resolver: &ReferenceResolver<'_>) -> Result<Self> {
        let spec = load_spec(path, &mut Vec::new(), resolver)?;
        Document::from_spec(spec.clone())?;
        Ok(Self {
            spec,
            context: current_context()?,
        })
    }
    pub fn instantiate(&self) -> Result<Document> {
        self.context.with(|| Document::from_spec(self.spec.clone()))
    }
    pub(crate) fn to_xml(&self) -> String {
        self.spec.to_xml()
    }
}
