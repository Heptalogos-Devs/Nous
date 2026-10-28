use super::*;
use std::collections::{BTreeMap,BTreeSet};

impl App {
    pub(super) async fn finish_import_files_in(&self,tx:&mut WriteTransaction,parent:&JobRecord,plan:&ParentPlan,result:&ImportProcessingResult)->Result<()> {
        let (namespace,subject)=parent.subject_scope()?;
        let catalog=tx.import_unit_catalog(namespace,subject,&result.catalog_id).await?;
        let units=tx.import_catalog_units(namespace,subject,&catalog.id).await?;
        let current=plan.request.unit_ids.as_ref().ok_or_else(||Error::internal("processing has no accepted unit selection"))?.iter().collect::<BTreeSet<_>>();
        let touched=units.iter().filter(|unit|current.contains(&unit.id)).map(|unit|unit.input_source_id.clone()).collect::<BTreeSet<_>>();
        for source_id in touched {
            let document=catalog.documents.iter().find(|document|document.input_source_id==source_id).ok_or_else(||Error::internal("processing file has no original document plan"))?;
            let file_units=units.iter().filter(|unit|unit.input_source_id==source_id).collect::<Vec<_>>();
            if file_units.len()!=document.original_unit_count {continue;}
            let source_refs=file_units.iter().flat_map(|unit|unit.source_requirements()).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
            match tx.require_source_requirements(&parent.call_context(&plan.caller_id)?,namespace,subject,&source_refs).await {
                Ok(())=>{},Err(error)if matches!(error.code,ErrorCode::Forbidden|ErrorCode::NotFound)=>continue,Err(error)=>return Err(error),
            }
            let ids=file_units.iter().map(|unit|unit.id.clone()).collect::<Vec<_>>();
            let mut candidates=tx.import_unit_output_candidates(namespace,subject,&ids).await?;
            candidates.sort_by(|a,b|b.commit_seq.cmp(&a.commit_seq).then(b.id.cmp(&a.id)));
            let mut selected=BTreeMap::<ImportUnitId,ImportFileUnitOutput>::new();
            for output in candidates {
                if selected.contains_key(&output.unit_id) {continue;}
                let child=match tx.job_record(&output.attempt_job_id).await {Ok(child)=>child,Err(error)if error.code==ErrorCode::NotFound=>continue,Err(error)=>return Err(error)};
                if child.kind!="import.unit"||child.state!=JobState::Succeeded||child.namespace_id.as_ref()!=Some(namespace)||child.subject_id.as_ref()!=Some(subject) {continue;}
                if child.require_read(&parent.call_context(&plan.caller_id)?).is_err(){continue;}
                let Some(checkpoint)=child.checkpoint else{continue;};
                let unit_plan:UnitPlan=serde_json::from_value(checkpoint)?;
                if unit_plan.catalog_id!=catalog.id||unit_plan.unit_id!=output.unit_id||unit_plan.origin!=plan.request.origin {continue;}
                let Some(value)=child.result else{continue;};
                let adopted:ImportUnitProcessResult=serde_json::from_value(value)?;
                if adopted.output_id!=output.id {continue;}
                let observation=tx.observation(namespace,&output.observation_id).await?;
                if observation.subject_id!=*subject||observation.kind!=ObservationKind::Import||observation.origin!=plan.request.origin {continue;}
                selected.insert(output.unit_id.clone(),ImportFileUnitOutput{unit_id:output.unit_id,output_id:output.id,vectors:unit_plan.vectors});
            }
            if selected.len()!=document.original_unit_count {continue;}
            let publication=plan.files.iter().find(|file|file.input_source_id==source_id);
            let dedupe_policy=publication.map_or(ImportFileDedupePolicy::None,|file|file.dedupe_policy);
            let file_identity=publication.and_then(|file|file.file_identity.clone());
            let fact=ImportFileSuccess{id:ImportFileSuccessId::new(),namespace_id:namespace.clone(),subject_id:subject.clone(),input_source_id:source_id.clone(),catalog_id:catalog.id.clone(),file_identity:file_identity.clone(),origin:plan.request.origin.clone(),
                unit_outputs:file_units.iter().map(|unit|selected.remove(&unit.id).expect("complete file units were checked")).collect(),dedupe_policy,processing_job_id:parent.id.clone(),recorded_at:tx.time().recorded(),commit_seq:tx.commit_seq()};
            tx.insert_import_file_success(&fact).await?;
            let source=tx.import_input_source(namespace,subject,&source_id).await?;
            if let Some(key)=dedupe_policy.key(&source.content_hash,file_identity.as_deref()) {
                tx.set_import_manifest_head(&ImportManifestHead{namespace_id:namespace.clone(),subject_id:subject.clone(),origin:plan.request.origin.clone(),policy:dedupe_policy,key,success_id:fact.id,recorded_at:tx.time().recorded(),commit_seq:tx.commit_seq()}).await?;
            }
        }
        Ok(())
    }
}
