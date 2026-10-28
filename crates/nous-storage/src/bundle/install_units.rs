use super::remap::Remap;
use crate::WriteTransaction;
use nous_domain::*;
use std::collections::{BTreeSet, VecDeque};

impl WriteTransaction {
    pub(super) async fn install_import_units(
        &mut self,
        package: &SubjectPackage,
        installation: &BundleInstallation,
        map: &Remap,
    ) -> Result<()> {
        let mut pending: VecDeque<_> = package.unit_catalogs.iter().collect();
        let mut installed = BTreeSet::new();
        while !pending.is_empty() {
            let count = pending.len();
            for _ in 0..count {
                let source = pending.pop_front().expect("pending catalog");
                if source
                    .parent_catalog_id
                    .as_ref()
                    .is_some_and(|id| !installed.contains(id))
                {
                    pending.push_back(source);
                    continue;
                }
                let value = map.unit_catalog(source)?;
                self.insert_import_unit_catalog(&value).await?;
                self.resource(
                    installation,
                    "import_unit_catalog",
                    source.id.as_str(),
                    value.id.as_str(),
                    None,
                    &source.recorded_at,
                    source.commit_seq,
                    None,
                )
                .await?;
                installed.insert(source.id.clone());
            }
            if pending.len() == count {
                return Err(Error::invalid(
                    "catalog parent headers do not form a complete acyclic graph",
                ));
            }
        }
        for source in &package.text_units {
            let value = map.text_unit(source)?;
            self.insert_import_text_unit(&value).await?;
            self.resource(
                installation,
                "import_text_unit",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
        }
        for source in &package.unit_outputs {
            let value = map.unit_output(source)?;
            self.insert_import_unit_output(&value).await?;
            self.resource(
                installation,
                "import_unit_output",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
        }
        Ok(())
    }
}
