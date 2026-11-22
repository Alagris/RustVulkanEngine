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

    assert!(
        base_ids && patch_ids,
        "id-keyed merging requires both sequences to be fully id-keyed"
    );
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
    debug_assert!(
        sequence_ids(&base).is_ok_and(|ids| ids) || base.is_empty(),
        "merged id-keyed sequence must keep unique non-empty ids"
    );
    debug_assert!(
        !base.iter().any(is_removal_marker),
        "merged sequence must not retain removal markers"
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(source: &str) -> Value {
        serde_yaml::from_str(source).unwrap()
    }

    #[test]
    fn merges_nested_mappings_recursively_and_replaces_scalars() {
        let merged = merge_values(
            yaml("name: Base\ntransform:\n  translation: [0, 0, 0]\n  scale: [1, 1, 1]\n"),
            yaml("name: Child\ntransform:\n  translation: [5, 0, 0]\n"),
        )
        .unwrap();
        assert_eq!(merged["name"].as_str(), Some("Child"));
        assert_eq!(merged["transform"]["translation"][0].as_i64(), Some(5));
        assert_eq!(
            merged["transform"]["scale"][1].as_i64(),
            Some(1),
            "unpatched nested keys must be inherited"
        );
    }

    #[test]
    fn patch_replaces_values_of_different_kinds() {
        assert_eq!(
            merge_values(yaml("[1, 2]"), yaml("value: 3")).unwrap(),
            yaml("value: 3")
        );
        assert_eq!(merge_values(yaml("scalar"), Value::Null).unwrap(), Value::Null);
    }

    #[test]
    fn non_id_sequences_are_replaced_not_merged() {
        let merged = merge_values(yaml("items: [1, 2, 3]"), yaml("items: [4]")).unwrap();
        assert_eq!(merged["items"], yaml("[4]"));
        let merged = merge_values(yaml("items: [1, 2]"), yaml("items: []")).unwrap();
        assert_eq!(merged["items"], yaml("[]"));
    }

    #[test]
    fn id_keyed_sequences_merge_append_and_remove_entries() {
        let merged = merge_values(
            yaml("children:\n  - id: a\n    name: First\n  - id: b\n    name: Second\n"),
            yaml(
                "children:\n  - id: a\n    name: Patched\n  - id: b\n    remove: true\n  - id: c\n    name: Added\n",
            ),
        )
        .unwrap();
        let children = merged["children"].as_sequence().unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["name"].as_str(), Some("Patched"));
        assert_eq!(children[1]["id"].as_str(), Some("c"));
        assert!(
            children.iter().all(|child| child.get("remove").is_none()),
            "removal markers must not leak into merged output"
        );
    }

    #[test]
    fn rejects_removal_of_unknown_inherited_entries() {
        let error = merge_values(
            yaml("children:\n  - id: a\n"),
            yaml("children:\n  - id: missing\n    remove: true\n"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot remove unknown inherited entry 'missing'")
        );
    }

    #[test]
    fn rejects_removal_markers_without_id_keyed_entries() {
        for (base, patch) in [
            ("items: []", "items:\n  - remove: true\n"),
            ("items: [1]", "items:\n  - remove: true\n"),
        ] {
            let error = merge_values(yaml(base), yaml(patch)).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("sequence removal entries require id-keyed inherited entries"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_empty_and_non_boolean_sequence_metadata() {
        let error = merge_values(
            yaml("items:\n  - id: a\n  - id: a\n"),
            yaml("items:\n  - id: a\n"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("id-keyed sequence contains duplicate entry identifiers")
        );

        let error = merge_values(
            yaml("items:\n  - id: a\n  - name: missing-id\n"),
            yaml("items:\n  - id: a\n"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("id-keyed sequence entries must have a non-empty string id")
        );

        let error = merge_values(
            yaml("items:\n  - id: a\n"),
            yaml("items:\n  - id: \"  \"\n"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("id-keyed sequence entries must have a non-empty string id")
        );

        let error = merge_values(
            yaml("items:\n  - id: a\n"),
            yaml("items:\n  - id: a\n    remove: \"yes\"\n"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("sequence remove markers must be booleans")
        );
    }
}
