use crate::App;
use nous_domain::*;
use nous_foundation::{clock::Clock, configuration::ConfigSnapshot};
use serde_json::{Value, json};
use std::collections::BTreeMap;

impl App {
    pub(super) async fn subject_runtime_diagnostics(
        &self,
        snapshot: &ConfigSnapshot,
        ctx: &CallContext,
    ) -> Result<Value> {
        let mut roles = BTreeMap::new();
        let mut configured_space = None;
        for definition in self.model.resources().roles().definitions() {
            let readiness = self
                .model
                .resources()
                .readiness(snapshot, definition.id)
                .await;
            let resolved = self.model.resources().resolve(snapshot, definition.id);
            let routes=match &resolved {
                Ok(model)=>model.routes.iter().map(|route|json!({"id":route.id,"resource":route.resource,"parameters":route.parameters})).collect::<Vec<_>>(),
                Err(_)=>vec![],
            };
            if definition.id == crate::retrieval::embedding_model::ROLE
                && let Ok(model) = &resolved
            {
                configured_space = Some(model.space()?);
            }
            roles.insert(definition.id,json!({"resource_ready":readiness.is_ready(),"readiness":readiness,"routes":routes}));
        }
        Ok(json!({
            "observed_at":Clock::operation_snapshot().recorded(),
            "data_dir":if ctx.require_admin().is_ok() {Some(self.paths.root.display().to_string())}else {None},
            "configuration_revision":snapshot.revision,
            "configuration_digest":snapshot.effective_digest,
            "configuration":{
                "retrieval":snapshot.get(crate::retrieval::config::RETRIEVAL)?,
                "integration":snapshot.get(crate::integrations::amemorix::AMEMORIX)?
            },
            "roles":roles,
            "embedding_space":configured_space,
            "durability":"transactional",
            "provider_probe_performed":false
        }))
    }
}
