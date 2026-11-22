use anyhow::{Result, anyhow, bail};
use serde_yaml::Value;
use std::collections::HashSet;

pub fn merge_values(base: Value, patch: Value) -> Result<Value> {
    match (base, patch) {
        (Value::Mapping(mut base), Value::Mapping(patch)) => {
            for (key, value) in patch {
                let merged = match base.remove(&key) {
                    Some(previous) => merge_values(previous, value)?,
                    None => value,
                };
                base.insert(key, merged);
            }
            Ok(Value::Mapping(base))
        }
        (Value::Sequence(base), Value::Sequence(patch)) => {
            merge_sequences(base, patch).map(Value::Sequence)
        }
        (_, patch) => Ok(patch),
    }
}

fn merge_sequences(mut base: Vec<Value>, patch: Vec<Value>) -> Result<Vec<Value>> {
    if base.is_empty() || patch.is_empty() {
        if patch.iter().any(is_removal_marker) {
            bail!("sequence removal entries require id-keyed inherited entries");
        }
        return Ok(patch);
    }
    let base_ids = sequence_ids(&base)?;
    let patch_ids = sequence_ids(&patch)?;
    if !base_ids && !patch_ids {
        if patch.iter().any(is_removal_marker) {
            bail!("sequence removal entries require id-keyed inherited entries");
        }
        return Ok(patch);
    }
    if !base_ids || !patch_ids {
        if patch.iter().any(is_removal_marker) {
            bail!("sequence removal entries require id-keyed inherited entries");
        }
        return Ok(patch);
    }

    for mut item in patch {
        let id = item_id(&item)?.to_owned();
        let remove = item
            .as_mapping_mut()
            .and_then(|mapping| mapping.remove(Value::String("remove".to_owned())))
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| anyhow!("sequence remove markers must be booleans"))
            })
            .transpose()?
            .unwrap_or(false);
        if remove {
            let Some(index) = base
                .iter()
                .position(|candidate| item_id(candidate).ok() == Some(id.as_str()))
            else {
                bail!("cannot remove unknown inherited entry '{id}'");
            };
            base.remove(index);
            continue;
        }
        if let Some(mapping) = item.as_mapping_mut() {
            mapping.remove(Value::String("remove".to_owned()));
        }
        if let Some(previous) = base
            .iter_mut()
            .find(|candidate| item_id(candidate).ok() == Some(id.as_str()))
        {
            *previous = merge_values(previous.clone(), item)?;
        } else {
            base.push(item);
        }
    }
    Ok(base)
}

fn sequence_ids(values: &[Value]) -> Result<bool> {
    let has_any_id = values.iter().any(|value| value.get("id").is_some());
    if !has_any_id {
        return Ok(false);
    }
    let mut ids = HashSet::new();
    for value in values {
        let id = item_id(value)?;
        if !ids.insert(id) {
            bail!("id-keyed sequence contains duplicate entry identifiers");
        }
    }
    Ok(true)
}

fn item_id(value: &Value) -> Result<&str> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("id-keyed sequence entries must have a non-empty string id"))
}

fn is_removal_marker(value: &Value) -> bool {
    value.get("remove").and_then(Value::as_bool) == Some(true)
}
