use super::op_id;
use crate::http::media::MediaOperation;
use anyhow::{Context, Result};
use clap::Subcommand;
use nous_domain::{MaterialId, ObservationId, SubjectId, media::*};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum MediaCommand {
    /// Show the complete media operation JSON Schema without connecting.
    Schema,
    /// Inspect original content type, dimensions, source and recorded metadata.
    Inspect { material: MaterialId },
    /// Resolve an exact original, region or representation with its source roots.
    Source {
        #[arg(long)]
        subject: SubjectId,
        /// JSON MediaReference; use media schema to inspect its shape.
        reference: String,
    },
    /// Expand every exact input and original source in the derivation graph.
    Lineage {
        #[arg(long)]
        subject: SubjectId,
        reference: String,
    },
    /// Start a persistent media interpretation job from a real source event.
    Derive {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        observation: ObservationId,
        #[arg(long, required_unless_present = "source", conflicts_with = "source")]
        material: Option<MaterialId>,
        /// Exact JSON MediaReference, for a selected region or prior representation.
        #[arg(long, required_unless_present = "material")]
        source: Option<String>,
        #[arg(long, default_value="description-only", value_parser=parse_strategy)]
        strategy: DerivationStrategy,
        #[arg(long, default_value="native", value_parser=parse_delivery)]
        delivery: MediaDelivery,
        /// Exact JSON RepresentationRef to revise, preserving the same identity.
        #[arg(long)]
        revise: Option<String>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Read preparation, description, result and job state.
    Run { job: String },
    /// Read one immutable representation revision.
    Representation {
        #[arg(long)]
        subject: SubjectId,
        id: DerivedRepresentationId,
        #[arg(long)]
        revision: i64,
    },
    History {
        #[arg(long)]
        subject: SubjectId,
        id: DerivedRepresentationId,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Region {
        #[command(subcommand)]
        command: RegionCommand,
    },
    /// Submit any typed media operation saved in a JSON file.
    Apply { file: PathBuf },
}

#[derive(Subcommand)]
pub enum RegionCommand {
    CreateSource {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
        #[arg(long)]
        parent: Option<SourceRegionId>,
        /// JSON SourceCoordinate: text span, image rectangle, time span, frame or page.
        #[arg(long)]
        coordinate: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    CreateDerived {
        #[arg(long)]
        subject: SubjectId,
        /// Exact JSON RepresentationRef.
        #[arg(long)]
        representation: String,
        #[arg(long)]
        parent: Option<DerivedRegionId>,
        /// JSON DerivedSelector selecting text, interpretation content or an artifact region.
        #[arg(long)]
        selector: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Source {
        #[arg(long)]
        subject: SubjectId,
        id: SourceRegionId,
    },
    Derived {
        #[arg(long)]
        subject: SubjectId,
        id: DerivedRegionId,
    },
}

impl MediaCommand {
    pub fn operation(self) -> Result<Option<MediaOperation>> {
        Ok(Some(match self {
            Self::Schema => return Ok(None),
            Self::Inspect { material } => MediaOperation::Inspect {
                material_id: material,
            },
            Self::Source { subject, reference } => MediaOperation::Source {
                subject_id: subject,
                reference: json(&reference)?,
            },
            Self::Lineage { subject, reference } => MediaOperation::Lineage {
                subject_id: subject,
                reference: json(&reference)?,
            },
            Self::Derive {
                subject,
                observation,
                material,
                source,
                strategy,
                delivery,
                revise,
                operation_id,
            } => {
                let source = match (material, source) {
                    (Some(material_id), None) => MediaReference::Original { material_id },
                    (None, Some(source)) => json(&source)?,
                    _ => anyhow::bail!("choose --material or --source"),
                };
                MediaOperation::Derive {
                    operation_id: op_id(operation_id),
                    request: DeriveMediaRequest {
                        subject_id: subject,
                        observation_id: observation,
                        source,
                        strategy,
                        delivery,
                        revise: revise.map(|value| json(&value)).transpose()?,
                    },
                }
            }
            Self::Run { job } => MediaOperation::Run { job_id: job },
            Self::Representation {
                subject,
                id,
                revision,
            } => MediaOperation::Representation {
                subject_id: subject,
                reference: RepresentationRef { id, revision },
            },
            Self::History { subject, id, limit } => MediaOperation::History {
                subject_id: subject,
                representation_id: id,
                limit,
            },
            Self::Region { command } => command.operation()?,
            Self::Apply { file } => serde_json::from_reader(
                std::fs::File::open(&file)
                    .with_context(|| format!("cannot open {}", file.display()))?,
            )
            .context("invalid media operation; inspect media schema")?,
        }))
    }
}
impl RegionCommand {
    fn operation(self) -> Result<MediaOperation> {
        Ok(match self {
            Self::CreateSource {
                subject,
                material,
                parent,
                coordinate,
                operation_id,
            } => MediaOperation::CreateSourceRegion {
                operation_id: op_id(operation_id),
                request: SourceRegionRequest {
                    subject_id: subject,
                    material_id: material,
                    parent,
                    coordinate: json(&coordinate)?,
                },
            },
            Self::CreateDerived {
                subject,
                representation,
                parent,
                selector,
                operation_id,
            } => MediaOperation::CreateDerivedRegion {
                operation_id: op_id(operation_id),
                request: DerivedRegionRequest {
                    subject_id: subject,
                    representation: json(&representation)?,
                    parent,
                    selector: json(&selector)?,
                },
            },
            Self::Source { subject, id } => MediaOperation::SourceRegion {
                subject_id: subject,
                id,
            },
            Self::Derived { subject, id } => MediaOperation::DerivedRegion {
                subject_id: subject,
                id,
            },
        })
    }
}
fn json<T: serde::de::DeserializeOwned>(input: &str) -> Result<T> {
    serde_json::from_str(input).context("invalid media JSON; inspect media schema")
}
fn parse_strategy(input: &str) -> Result<DerivationStrategy, String> {
    match input {
        "description-only" => Ok(DerivationStrategy::DescriptionOnly),
        "direct-structured" => Ok(DerivationStrategy::DirectStructured),
        "describe-then-structure" => Ok(DerivationStrategy::DescribeThenStructure),
        _ => Err("choose description-only, direct-structured or describe-then-structure".into()),
    }
}
fn parse_delivery(input: &str) -> Result<MediaDelivery, String> {
    match input {
        "native" => Ok(MediaDelivery::Native),
        "sampled-frames" => Ok(MediaDelivery::SampledFrames),
        _ => Err("choose native or sampled-frames".into()),
    }
}
