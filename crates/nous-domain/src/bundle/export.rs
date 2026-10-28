use crate::{BundleContentLevel, ConversationRef, Error, ExportBundleRequest, MaterialId, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundleExportFormat {
    Native,
    Amemorix,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundleSelector {
    All,
    Chat {
        conversation_ref: ConversationRef,
    },
    Source {
        sources: Vec<String>,
    },
    Package {
        package_id: Option<String>,
        installation_id: Option<String>,
    },
    PackageToken {
        token: String,
    },
    ImportTask {
        job_id: String,
    },
    Materials {
        material_ids: Vec<MaterialId>,
    },
    Images {
        content_hashes: Vec<String>,
        occurrence_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleAssetOptions {
    pub include_vectors: bool,
    pub include_images: bool,
    pub include_image_related: bool,
}
impl Default for BundleAssetOptions {
    fn default() -> Self {
        Self {
            include_vectors: false,
            include_images: true,
            include_image_related: false,
        }
    }
}

impl ExportBundleRequest {
    pub fn validate(&self) -> Result<()> {
        self.package.validate()?;
        if self.format == BundleExportFormat::Native
            && (self.content_level != BundleContentLevel::Full || self.assets.is_some())
        {
            return Err(Error::invalid(
                "native subject archives retain full history and all source assets; use amemorix for a portable projection",
            ));
        }
        if self.format == BundleExportFormat::Amemorix && self.package.version.is_none() {
            return Err(Error::invalid(
                "A-Memorix exports require a package release version",
            ));
        }
        match &self.selector {
            BundleSelector::Chat { conversation_ref } => conversation_ref.validate()?,
            BundleSelector::Source { sources } => {
                if sources.is_empty() {
                    return Err(Error::invalid("bundle source selection is empty"));
                }
                for source in sources {
                    crate::validate_name(source, "bundle source")?;
                }
            }
            BundleSelector::Package {
                package_id,
                installation_id,
            } => match (package_id, installation_id) {
                (Some(id), None) => crate::validate_name(id, "bundle package")?,
                (None, Some(id)) => crate::validate_name(id, "bundle installation")?,
                _ => {
                    return Err(Error::invalid(
                        "select exactly one package ID or installation ID",
                    ));
                }
            },
            BundleSelector::ImportTask { job_id } => {
                crate::validate_name(job_id, "bundle import task")?
            }
            BundleSelector::PackageToken { token } => {
                crate::validate_name(token, "bundle package token")?
            }
            BundleSelector::Materials { material_ids } => {
                if material_ids.is_empty() {
                    return Err(Error::invalid("bundle material selection is empty"));
                }
                for id in material_ids {
                    id.validate()?;
                }
            }
            BundleSelector::Images {
                content_hashes,
                occurrence_ids,
            } if content_hashes.is_empty() && occurrence_ids.is_empty() => {
                return Err(Error::invalid("bundle image selection is empty"));
            }
            _ => {}
        }
        Ok(())
    }
}
