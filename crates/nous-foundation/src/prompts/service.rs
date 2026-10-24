use super::*;
use crate::{clock::Clock, instance::InstancePaths};
use nous_domain::{
    CallContext, ConfigScope, Error, ErrorCode, PromptRef, PromptRevision, Result, validate_name,
    validate_prompt_locale,
};
use nous_storage::{Database, WriteTransaction};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct Prompts {
    paths: InstancePaths,
    database: Database,
    clock: Clock,
    bundled: Arc<BTreeMap<String, &'static str>>,
}
impl Prompts {
    pub fn new(
        paths: InstancePaths,
        database: Database,
        clock: Clock,
        definitions: impl IntoIterator<Item = PromptDefinition>,
    ) -> Self {
        Self {
            paths,
            database,
            clock,
            bundled: Arc::new(
                definitions
                    .into_iter()
                    .map(|definition| (definition.id.into(), definition.template))
                    .collect(),
            ),
        }
    }
    pub async fn read(
        &self,
        scope: &ConfigScope,
        default_id: &str,
        binding: &PromptBinding,
    ) -> Result<PromptTemplate> {
        binding.validate_for_scope(scope)?;
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        let managed = if let PromptBinding::Managed { reference } = binding {
            Some(view.prompt_revision(reference).await?)
        } else {
            None
        };
        view.finish().await?;
        if let Some(revision) = managed {
            return template(revision);
        }
        self.nonmanaged(default_id, binding).await
    }
    pub async fn read_in(
        &self,
        tx: &mut WriteTransaction,
        scope: &ConfigScope,
        default_id: &str,
        binding: &PromptBinding,
    ) -> Result<PromptTemplate> {
        binding.validate_for_scope(scope)?;
        tx.validate_configuration_scope(scope).await?;
        if let PromptBinding::Managed { reference } = binding {
            return template(tx.prompt_revision(reference).await?);
        }
        self.nonmanaged(default_id, binding).await
    }
    async fn nonmanaged(
        &self,
        default_id: &str,
        binding: &PromptBinding,
    ) -> Result<PromptTemplate> {
        match binding {
            PromptBinding::Default => {
                let text = self
                    .bundled
                    .get(default_id)
                    .ok_or_else(|| Error::new(ErrorCode::NotFound, "bundled prompt was not found"))?
                    .to_string();
                let variables = validate_template(default_id, &text)?;
                Ok(PromptTemplate {
                    id: default_id.into(),
                    binding: PromptBinding::Default,
                    locale: None,
                    sha256: digest(&text),
                    text,
                    variables,
                })
            }
            PromptBinding::File { file, locale } => {
                let paths = self.paths.clone();
                let file = file.clone();
                let locale = locale.clone();
                tokio::task::spawn_blocking(move || files::template(&paths, &file, locale))
                    .await
                    .map_err(Error::dependency)?
            }
            PromptBinding::Managed { .. } => {
                unreachable!("managed resources are read in their authority snapshot")
            }
        }
    }
    pub async fn render(
        &self,
        scope: &ConfigScope,
        id: &str,
        binding: &PromptBinding,
        arguments: &PromptArguments,
    ) -> Result<PromptSnapshot> {
        let asset = self.read(scope, id, binding).await?;
        self.render_template(&asset, arguments)
    }
    pub fn render_template(
        &self,
        asset: &PromptTemplate,
        arguments: &PromptArguments,
    ) -> Result<PromptSnapshot> {
        render::render(asset, arguments, None)
    }
    pub fn preview_template(
        &self,
        asset: &PromptTemplate,
        arguments: &PromptArguments,
        override_text: Option<&str>,
    ) -> Result<PromptSnapshot> {
        render::render(asset, arguments, override_text)
    }
    pub async fn preview(
        &self,
        scope: &ConfigScope,
        id: &str,
        binding: &PromptBinding,
        arguments: &PromptArguments,
        override_text: Option<&str>,
    ) -> Result<PromptSnapshot> {
        let asset = self.read(scope, id, binding).await?;
        self.preview_template(&asset, arguments, override_text)
    }
    pub async fn edit_in(
        &self,
        tx: &mut WriteTransaction,
        ctx: &CallContext,
        scope: &ConfigScope,
        edit: PromptEdit,
    ) -> Result<PromptRevision> {
        scope.require_access(ctx)?;
        let PromptEdit {
            id,
            expected_revision,
            body,
            locale,
            reason,
        } = edit;
        validate_name(&id, "prompt id")?;
        validate_prompt_locale(locale.as_deref())?;
        validate_template(&id, &body)?;
        let revision = PromptRevision {
            reference: PromptRef {
                scope: scope.clone(),
                id,
                revision: tx.commit_seq(),
            },
            locale,
            body,
            author: ctx.caller_id.clone(),
            reason,
            recorded_at: self.clock.recorded(),
        };
        tx.insert_prompt_revision(&revision, expected_revision)
            .await?;
        Ok(revision)
    }
    pub async fn history(
        &self,
        scope: &ConfigScope,
        resource_scope: &ConfigScope,
        id: &str,
        limit: usize,
    ) -> Result<Vec<PromptRevision>> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::invalid("prompt history limit must be 1..1000"));
        }
        validate_inheritance(scope, resource_scope)?;
        let mut view = self.database.read_view().await?;
        view.validate_configuration_scope(scope).await?;
        let revisions = view.prompt_history(resource_scope, id, limit).await?;
        view.finish().await?;
        Ok(revisions)
    }
    pub async fn catalog(&self, scope: &ConfigScope, limit: usize) -> Result<PromptCatalog> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::invalid("prompt catalog limit must be 1..1000"));
        }
        let managed = self
            .database
            .prompt_catalog(&hierarchy(scope), limit + 1)
            .await?;
        let mut resources = Vec::new();
        for (id, text) in self.bundled.iter() {
            resources.push(PromptCatalogEntry {
                id: id.clone(),
                binding: PromptBinding::Default,
                locale: None,
                byte_len: Some(text.len()),
                variables: validate_template(id, text)?,
                error: None,
            });
        }
        let paths = self.paths.clone();
        let (files, files_complete) =
            tokio::task::spawn_blocking(move || files::catalog(&paths, limit + 1))
                .await
                .map_err(Error::dependency)??;
        resources.extend(files);
        for revision in managed {
            resources.push(PromptCatalogEntry {
                id: revision.reference.id.clone(),
                binding: PromptBinding::Managed {
                    reference: revision.reference,
                },
                locale: revision.locale,
                byte_len: Some(revision.body.len()),
                variables: validate_template("managed", &revision.body)?,
                error: None,
            });
        }
        let complete = files_complete && resources.len() <= limit;
        resources.sort_by(|a, b| a.id.cmp(&b.id));
        resources.truncate(limit);
        Ok(PromptCatalog {
            resources,
            complete,
        })
    }
    pub fn file_reference(
        paths: &InstancePaths,
        binding: &PromptBinding,
    ) -> Result<Option<ManagedPromptReference>> {
        binding.validate()?;
        files::reference(paths, binding)
    }
    pub async fn validate_managed_bindings(
        database: &Database,
        bindings: &[PromptBinding],
    ) -> Result<()> {
        let mut view = database.read_view().await?;
        for binding in bindings {
            binding.validate()?;
            if let PromptBinding::Managed { reference } = binding {
                view.prompt_revision(reference).await?.validate()?;
            }
        }
        view.finish().await?;
        Ok(())
    }
}
fn template(revision: PromptRevision) -> Result<PromptTemplate> {
    let variables = validate_template(&revision.reference.id, &revision.body)?;
    Ok(PromptTemplate {
        id: revision.reference.id.clone(),
        binding: PromptBinding::Managed {
            reference: revision.reference,
        },
        locale: revision.locale,
        sha256: digest(&revision.body),
        text: revision.body,
        variables,
    })
}
