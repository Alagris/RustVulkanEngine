use super::data_asset_cell::DataAssetValue;
use std::sync::Weak;

/// Non-owning access to a uniquely owned data asset.
#[derive(Clone)]
pub struct DataAssetLink {
    value: Weak<DataAssetValue>,
}

impl DataAssetLink {
    pub(crate) fn new(value: Weak<DataAssetValue>) -> Self {
        Self { value }
    }

    pub fn is_alive(&self) -> bool {
        self.with_value(|_| ()).is_some()
    }

    pub(crate) fn with_value<R>(&self, action: impl FnOnce(&DataAssetValue) -> R) -> Option<R> {
        // The temporary lease makes dropping the owner inside a callback safe.
        // It is never exposed or stored in another asset.
        let value = self.value.upgrade()?;
        if !value.is_alive() {
            return None;
        }
        Some(action(&value))
    }
}
