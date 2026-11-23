#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct DataAssetKey {
    pub(crate) type_name: String,
    pub(crate) name: String,
}
