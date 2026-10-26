use super::*;
impl App {
    pub async fn context_residents(
        &self,
        ctx: &CallContext,
        id: &ContextSessionId,
        include_evicted: bool,
    ) -> Result<Vec<ContextResident>> {
        self.get_context_session(ctx, id).await?;
        self.database.context_residents(id, include_evicted).await
    }
    pub async fn change_context_residents(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &ContextSessionId,
        expected_revision: i64,
        change: ContextResidentChange,
    ) -> Result<OperationReceipt> {
        let session = self.get_context_session(ctx, id).await?;
        let key = operation_key(
            ctx,
            "context.residents",
            &scope(&session.namespace_id, &session.subject_id),
            operation_id,
            &json!({"session_id":id,"expected_revision":expected_revision,"change":change}),
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let limit = self
            .context_resident_limit(&session.namespace_id, &session.subject_id)
            .await?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let outcome = match change {
            ContextResidentChange::Admit { items } => {
                tx.apply_context_residents(id, expected_revision, &items, limit)
                    .await?
            }
            ContextResidentChange::Evict { targets } => {
                tx.evict_context_residents(id, expected_revision, Some(&targets))
                    .await?
            }
            ContextResidentChange::Clear => {
                tx.evict_context_residents(id, expected_revision, None)
                    .await?
            }
        };
        tx.commit(json!(outcome)).await
    }
}
