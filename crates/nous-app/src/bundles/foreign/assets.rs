//! Preserve validated external assets without claiming a local execution or acceptance.
use super::*;

impl Builder<'_> {
    pub(super) fn retained_assets(
        &mut self,
        inspection: &BundleInspection,
        object_members: &mut BTreeMap<String, String>,
    ) -> Result<Vec<String>> {
        let mut unaccepted = Vec::new();
        for member in inspection.index.vector_members.values() {
            self.retain_asset(
                inspection,
                member,
                "external_embedding_archive",
                object_members,
            )?;
            unaccepted.push(member.clone());
        }
        for member in &inspection.index.extension_members {
            let source_type = match member.as_str() {
                "nous-vector-spaces.json" => "nous_portable_vector_metadata",
                "nous-semantic.json" => {
                    if inspection.manifest.content_level != crate::bundle_format::ContentLevel::Full
                    {
                        return Err(Error::invalid(
                            "Nous semantic projection requires a full portable bundle",
                        ));
                    }
                    "nous_portable_semantic_projection"
                }
                _ if (member.starts_with("vectors/nous/")
                    || member.starts_with("vectors/external/"))
                    && member.ends_with(".npz") =>
                {
                    unaccepted.push(member.clone());
                    "nous_portable_vector_archive"
                }
                _ => continue,
            };
            self.retain_asset(inspection, member, source_type, object_members)?;
        }
        Ok(unaccepted)
    }

    fn retain_asset(
        &mut self,
        inspection: &BundleInspection,
        member: &str,
        source_type: &str,
        object_members: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        let source = inspection.members.get(member).ok_or_else(|| {
            Error::invalid("retained asset is missing from validated bundle inventory")
        })?;
        let (namespace_id, subject_id) = self.scope();
        let id = MaterialId::new();
        let media_type = if member.ends_with(".npz") {
            "application/x-npz"
        } else {
            "application/json"
        };
        self.package.materials.push(MediaOriginal {
            material: Material {
                id: id.clone(),
                namespace_id: namespace_id.clone(),
                content_hash: source.sha256.clone(),
                media_type: media_type.into(),
                metadata: MediaMetadata::Binary,
                byte_len: i64::try_from(source.byte_len)
                    .map_err(|_| Error::invalid("bundle asset length exceeds storage range"))?,
                source: Some(format!("a-memorix:{member}")),
                event_time: None,
                recorded_at: self.package.created_at.clone(),
                commit_seq: 1,
            },
            details: MaterialDetails {
                source_type: Some(source_type.into()),
                tags: vec![],
                metadata: json!({
                    "member":member,
                    "embedding_fingerprint":inspection.manifest.embedding_fingerprint,
                    "image_embedding_fingerprint":inspection.manifest.image_embedding_fingerprint,
                    "adoption":"retained_source_not_adopted_as_native_state",
                }),
                structured: None,
            },
            details_redacted: false,
        });
        self.package.observations.push(Observation {
            id: ObservationId::new(),
            namespace_id,
            subject_id,
            material_id: id,
            observed_at: None,
            origin: None,
            kind: ObservationKind::Import,
            recorded_at: self.package.created_at.clone(),
            commit_seq: 1,
        });
        object_members.insert(source.sha256.clone(), member.into());
        Ok(())
    }
}
