use super::{op_id, parse_time};
use crate::http::{Operation, RecordObservation, sources::RetrievalSourceRequest};
use anyhow::Result;
use clap::Subcommand;
use nous_domain::{
    ConversationRef, ExternalObjectRef, MaterialId, ObservationId, ObservationOrigin,
    RetrievalTarget, SubjectId,
};

#[derive(Subcommand)]
pub enum ObservationCommand {
    Record {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
        /// JSON time point; omitted means the contact time was not supplied.
        #[arg(long)]
        observed_at: Option<String>,
        #[arg(long)]
        conversation_ref: Option<ConversationRef>,
        #[arg(long)]
        external_object_ref: Option<ExternalObjectRef>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List {
        #[arg(long)]
        subject: SubjectId,
    },
}
impl ObservationCommand {
    pub fn operation(self) -> Result<Operation> {
        Ok(match self {
            Self::Record {
                subject,
                material,
                observed_at,
                conversation_ref,
                external_object_ref,
                operation_id,
            } => Operation::RecordObservation(RecordObservation {
                operation_id: op_id(operation_id),
                subject_id: subject.to_string(),
                material_id: material.to_string(),
                observed_at: parse_time(observed_at)?,
                origin: ObservationOrigin::from_refs(conversation_ref, external_object_ref)?,
            }),
            Self::List { subject } => Operation::ListObservations(subject.to_string()),
        })
    }
}
#[derive(Subcommand)]
pub enum SourceCommand {
    Read {
        reference: RetrievalTarget,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        observation: Option<ObservationId>,
    },
}
impl SourceCommand {
    pub fn operation(self) -> Operation {
        match self {
            Self::Read {
                reference,
                subject,
                observation,
            } => Operation::ReadSource(RetrievalSourceRequest {
                subject_id: subject,
                reference,
                observation_id: observation,
            }),
        }
    }
}
