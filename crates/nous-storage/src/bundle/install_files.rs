use super::remap::Remap;
use crate::WriteTransaction;
use nous_domain::*;

impl WriteTransaction {
    pub(super) async fn install_import_files(
        &mut self,
        package: &SubjectPackage,
        installation: &BundleInstallation,
        map: &Remap,
    ) -> Result<()> {
        for source in &package.file_successes {
            let value = map.file_success(source)?;
            self.insert_import_file_success(&value).await?;
            self.resource(
                installation,
                "import_file_success",
                source.id.as_str(),
                value.id.as_str(),
                None,
                &source.recorded_at,
                source.commit_seq,
                None,
            )
            .await?;
        }
        for source in &package.import_manifest_heads {
            let fact = package
                .file_successes
                .iter()
                .find(|fact| fact.id == source.success_id)
                .ok_or_else(|| {
                    Error::invalid("manifest qualification omits its actual file success")
                })?;
            let mut same_origin = true;
            for pair in &fact.unit_outputs {
                let output = package
                    .unit_outputs
                    .iter()
                    .find(|output| output.id == pair.output_id)
                    .ok_or_else(|| {
                        Error::invalid("manifest qualification omits actual adopted output")
                    })?;
                let event_id = ObservationId(map.id(output.observation_id.as_str())?);
                let actual = self
                    .observation(&installation.namespace_id, &event_id)
                    .await?;
                if actual.subject_id != installation.subject_id
                    || actual.kind != ObservationKind::Import
                {
                    return Err(Error::invalid(
                        "installed manifest output has no actual scoped Import event",
                    ));
                }
                if actual.origin != source.origin {
                    same_origin = false;
                    break;
                }
            }
            // Historical success survives a placement change; current context qualification does not.
            if same_origin {
                let value = map.manifest_head(source)?;
                self.insert_import_manifest_head_if_absent(&value).await?;
            }
        }
        Ok(())
    }
}
