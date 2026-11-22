use crate::data::data_asset::{
    DataAsset, DataAssetCell, DataAssetDeserializeContext, DataAssetTarget,
};
use anyhow::{Result, anyhow, bail};
use serde_yaml::Value;
use std::sync::Arc;

pub trait Modifier: DataAsset {}

pub type ModifierCast = for<'a> fn(&'a dyn DataAsset) -> Option<&'a dyn Modifier>;

pub struct ModifierRegistration {
    pub type_name: &'static str,
    pub cast: ModifierCast,
}

pub use inventory;
inventory::collect!(ModifierRegistration);

fn registration(type_name: &str) -> Option<&'static ModifierRegistration> {
    inventory::iter::<ModifierRegistration>
        .into_iter()
        .find(|registration| registration.type_name == type_name)
}

impl DataAssetTarget for dyn Modifier {
    fn type_name() -> Option<&'static str> {
        None
    }

    fn cast<'a>(asset: &'a dyn DataAsset, cell: &'a DataAssetCell) -> Option<&'a Self> {
        (registration(cell.registration.type_name)?.cast)(asset)
    }

    fn resolve_link(
        context: &DataAssetDeserializeContext,
        name: &str,
        type_name: Option<&str>,
    ) -> Result<(Arc<DataAssetCell>, String, String)> {
        let type_name = match type_name {
            Some(type_name) => {
                if registration(type_name).is_none() {
                    bail!("data asset type '{type_name}' does not implement Modifier");
                }
                type_name
            }
            None => {
                let mut candidates = inventory::iter::<ModifierRegistration>
                    .into_iter()
                    .filter(|registration| context.has_asset(name, registration.type_name));
                let registration = candidates
                    .next()
                    .ok_or_else(|| anyhow!("no modifier data asset named '{name}' exists"))?;
                if candidates.next().is_some() {
                    bail!("modifier data asset name '{name}' is ambiguous; specify its type");
                }
                registration.type_name
            }
        };
        context.resolve_asset_link(name, type_name)
    }

    fn deserialize_nested(
        context: &DataAssetDeserializeContext,
        type_name: Option<&str>,
        value: Value,
    ) -> Result<Arc<DataAssetCell>> {
        let type_name = type_name
            .ok_or_else(|| anyhow!("nested modifier data assets must specify a 'type'"))?;
        let registration = registration(type_name)
            .ok_or_else(|| anyhow!("data asset type '{type_name}' does not implement Modifier"))?;
        let cell = context.create_nested_asset(type_name, &value)?;
        let asset = cell
            .asset()
            .ok_or_else(|| anyhow!("nested modifier asset '{type_name}' was not initialized"))?;
        if (registration.cast)(asset).is_none() {
            bail!("data asset factory for '{type_name}' returned a non-modifier");
        }
        Ok(cell)
    }
}

#[cfg(test)]
mod tests {
    use crate::data::data_asset::DataRef;
    use crate::data::data_asset_database::DataAssetDatabase;
    use crate::data::{DataAsset, Modifier};
    use serde::{Deserialize, Serialize};
    use serde_yaml::Value;
    use std::path::PathBuf;

    #[derive(DataAsset, Debug, Deserialize, Serialize)]
    struct ModifierHolder {
        modifiers: Vec<DataRef<dyn Modifier>>,
    }

    #[derive(DataAsset, Modifier, Debug, Deserialize, Serialize)]
    struct TestModifier {
        factor: f32,
    }

    fn document(yaml: &str, filename: &str) -> (String, Value, PathBuf) {
        (
            filename.to_owned(),
            serde_yaml::from_str(yaml).unwrap(),
            PathBuf::from(filename),
        )
    }

    #[test]
    fn resolves_nested_and_linked_modifiers_without_modifier_registry_coupling() {
        let database = DataAssetDatabase::from_documents(vec![
            document(
                "type: ModifierHolder\nname: holder\ndata:\n  modifiers:\n    - { type: TestModifier, data: { factor: 2.0 } }\n    - { ref: linked }\n",
                "holder.data",
            ),
            document(
                "type: TestModifier\nname: linked\ndata:\n  factor: 3.0\n",
                "linked.data",
            ),
        ])
        .unwrap();
        let holder = database.get::<ModifierHolder>("holder").unwrap();
        assert_eq!(
            holder.with(|holder| holder.modifiers[0].is_nested()),
            Some(true)
        );
        assert_eq!(
            holder.with(|holder| holder.modifiers[1].is_nested()),
            Some(false)
        );
        assert_eq!(
            holder.with(|holder| {
                holder.modifiers[0].with(|modifier| {
                    modifier
                        .as_any()
                        .downcast_ref::<TestModifier>()
                        .unwrap()
                        .factor
                })
            }),
            Some(Some(2.0))
        );
        assert_eq!(
            holder.with(|holder| {
                holder.modifiers[1].with(|modifier| {
                    modifier
                        .as_any()
                        .downcast_ref::<TestModifier>()
                        .unwrap()
                        .factor
                })
            }),
            Some(Some(3.0))
        );
    }
}
