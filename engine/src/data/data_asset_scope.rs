use super::data_asset_deserialize_context::DataAssetDeserializeContext;
use anyhow::{Result, anyhow};
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    static ACTIVE_CONTEXTS: RefCell<Vec<Arc<DataAssetDeserializeContext>>> = const { RefCell::new(Vec::new()) };
}

struct ContextGuard(usize);

impl Drop for ContextGuard {
    fn drop(&mut self) {
        ACTIVE_CONTEXTS.with(|contexts| {
            let mut contexts = contexts.borrow_mut();
            assert_eq!(
                contexts.len(),
                self.0 + 1,
                "data asset context stack must remain balanced"
            );
            contexts
                .pop()
                .expect("active data asset context must exist");
        });
    }
}

pub(crate) fn with_context<R>(
    context: Arc<DataAssetDeserializeContext>,
    action: impl FnOnce() -> Result<R>,
) -> Result<R> {
    let depth = ACTIVE_CONTEXTS.with(|contexts| {
        let mut contexts = contexts.borrow_mut();
        let depth = contexts.len();
        contexts.push(context);
        depth
    });
    let _guard = ContextGuard(depth);
    action()
}

pub(crate) fn current_context() -> Result<Arc<DataAssetDeserializeContext>> {
    ACTIVE_CONTEXTS.with(|contexts| {
        contexts
            .borrow()
            .last()
            .cloned()
            .ok_or_else(|| anyhow!("DataRef can only be deserialized while loading a data asset"))
    })
}
