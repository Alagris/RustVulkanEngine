use super::data_asset::DataAsset;
use super::data_asset_link::DataAssetLink;
use anyhow::{Result, anyhow};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// A unique owner. Moving the box never invalidates its checked weak links.
pub struct DataAssetCell {
    value: Arc<DataAssetValue>,
}

pub(crate) struct DataAssetValue {
    pub(crate) name: Option<String>,
    pub(crate) type_name: &'static str,
    alive: AtomicBool,
    asset: OnceLock<Box<dyn DataAsset>>,
}

impl DataAssetCell {
    pub(crate) fn new(name: Option<String>, type_name: &'static str) -> Self {
        Self {
            value: Arc::new(DataAssetValue {
                name,
                type_name,
                alive: AtomicBool::new(true),
                asset: OnceLock::new(),
            }),
        }
    }

    pub fn name(&self) -> Option<&str> {
        self.value.name.as_deref()
    }

    pub fn type_name(&self) -> &'static str {
        self.value.type_name
    }

    pub fn link(&self) -> DataAssetLink {
        DataAssetLink::new(Arc::downgrade(&self.value))
    }

    pub(crate) fn value(&self) -> &DataAssetValue {
        &self.value
    }

    pub(crate) fn set(&self, asset: Box<dyn DataAsset>) -> Result<()> {
        self.value
            .asset
            .set(asset)
            .map_err(|_| anyhow!("data asset cell was initialized more than once"))
    }

    pub(crate) fn asset(&self) -> Option<&dyn DataAsset> {
        self.value.asset()
    }
}

impl Drop for DataAssetCell {
    fn drop(&mut self) {
        // Existing callbacks may finish, but no new access can outlive the unique owner.
        self.value.alive.store(false, Ordering::Release);
    }
}

impl DataAssetValue {
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(crate) fn asset(&self) -> Option<&dyn DataAsset> {
        self.asset.get().map(Box::as_ref)
    }
}
