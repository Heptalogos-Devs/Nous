use nous_domain::SourceReadScope;
use std::{path::PathBuf, sync::OnceLock};

static SOURCE_SCOPES: OnceLock<Vec<SourceReadScope>> = OnceLock::new();
pub(super) fn initialize(files: &[PathBuf]) -> anyhow::Result<()> {
    let mut scopes = Vec::new();
    for path in files {
        let scope: SourceReadScope = serde_json::from_reader(std::fs::File::open(path)?)?;
        scope.validate()?;
        scopes.push(scope);
    }
    SOURCE_SCOPES
        .set(scopes)
        .map_err(|_| anyhow::anyhow!("CLI source scope was already initialized"))
}
pub(crate) fn scopes() -> &'static [SourceReadScope] {
    SOURCE_SCOPES.get().map_or(&[], Vec::as_slice)
}
