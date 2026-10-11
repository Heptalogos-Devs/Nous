//! Stable model-facing aliases. Canonical object identity remains owner-owned.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::proquint::{decode_proquint, encode_proquint};
use crate::*;
use nous_core::{CognitiveRef, parse_reference, reference_parts};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityBinding {
    pub lexical_ref: String,
    pub canonical: CognitiveRef,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct IdentityAddressTarget {
    pub subject: SubjectId,
    pub reference: CognitiveRef,
}

#[derive(Debug, Clone)]
pub struct IdentityAddress {
    pub target: IdentityAddressTarget,
    pub lexical_ref: Option<String>,
    pub status: String,
}

pub fn lexical_prefix(kind: &str) -> Result<&'static str> {
    Ok(match kind {
        "entity" => "ent",
        "memory" => "mem",
        "memory_revision" => "memrev",
        "cognitive_schema" => "schema",
        "cognitive_schema_revision" => "schemarev",
        "tag" => "tag",
        "artifact" => "art",
        "occurrence" => "obs",
        "source_region" => "src",
        "derived_representation" => "repr",
        "derived_region" => "region",
        "session" => "session",
        "subject" => "sub",
        "work_context" => "ctx",
        "association" => "assoc",
        "episode" => "ep",
        "episode_revision" => "eprev",
        "journal" => "journal",
        "journal_revision" => "journalrev",
        "cognitive_seed_version" => "seed",
        "resource" => "res",
        _ => return Err(Error::Invalid("object kind has no lexical address".into())),
    })
}
pub fn validate_lexical(value: &str) -> Result<&str> {
    let (kind, body) = value
        .split_once(':')
        .ok_or_else(|| Error::Invalid("INVALID_LEXICAL_REF".into()))?;
    if !matches!(
        kind,
        "ent"
            | "mem"
            | "memrev"
            | "schema"
            | "schemarev"
            | "tag"
            | "art"
            | "obs"
            | "src"
            | "repr"
            | "region"
            | "session"
            | "sub"
            | "ctx"
            | "assoc"
            | "ep"
            | "eprev"
            | "journal"
            | "journalrev"
            | "seed"
            | "res"
    ) {
        return Err(Error::Invalid("INVALID_LEXICAL_REF".into()));
    }
    decode_proquint(body)?;
    Ok(kind)
}
impl AuthorityStore {
    /// Creation owners publish addresses within their own Authority transaction.
    pub async fn ensure_identity_addresses_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        label: &str,
    ) -> Result<()> {
        let mut display_name = String::new();
        for character in label.chars() {
            if display_name.len() + character.len_utf8() > 256 {
                break;
            }
            display_name.push(character);
        }
        let mut seen = std::collections::BTreeSet::new();
        for reference in references {
            if !seen.insert(reference_parts(reference)) {
                continue;
            }
            self.bind_identity_in_mode(
                tx,
                subject,
                reference.clone(),
                display_name.clone(),
                vec![],
                true,
            )
            .await?;
        }
        Ok(())
    }

    /// One directory read. Missing addresses remain missing; presentation never grants visibility.
    pub async fn identity_addresses(
        &self,
        targets: &[IdentityAddressTarget],
    ) -> Result<Vec<IdentityAddress>> {
        if targets.len() > 2048 {
            return Err(Error::Invalid(
                "identity address batch exceeds 2048 targets".into(),
            ));
        }
        let requested = targets
            .iter()
            .map(|target| {
                let (kind, value) = reference_parts(&target.reference);
                serde_json::json!({"subject":target.subject.0,"kind":kind,"value":value})
            })
            .collect::<Vec<_>>();
        let rows = sqlx::query(r#"
WITH requested AS (
 SELECT r.subject,r.kind,r.value,requested.ordinal,
   CASE WHEN r.kind='tag' THEN canonical_tag(r.subject,r.value::uuid)::text ELSE r.value END AS resolved
 FROM jsonb_array_elements($1) WITH ORDINALITY AS requested(target,ordinal)
 CROSS JOIN LATERAL jsonb_to_record(requested.target) AS r(subject uuid,kind text,value text)
)
SELECT r.subject,r.kind,r.value,
 CASE WHEN v.subject_id IS NOT NULL AND b.tombstoned_at IS NULL AND r.resolved IS NOT NULL THEN b.lexical_ref END AS lexical_ref,
 CASE WHEN v.subject_id IS NULL THEN 'UNKNOWN_REFERENCE'
      WHEN b.tombstoned_at IS NOT NULL OR r.resolved IS NULL THEN 'REFERENCE_TOMBSTONED'
      ELSE 'BOUND' END AS status
FROM requested r
LEFT JOIN lexical_bindings b ON b.object_kind=r.kind AND b.canonical_ref=COALESCE(r.resolved,r.value)
LEFT JOIN lexical_visibility v ON v.lexical_ref=b.lexical_ref AND v.subject_id=r.subject
ORDER BY r.ordinal
"#).bind(serde_json::json!(requested)).fetch_all(self.pool()).await.map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(IdentityAddress {
                    target: IdentityAddressTarget {
                        subject: SubjectId(row.try_get("subject").map_err(database_error)?),
                        reference: parse_reference(
                            &row.try_get::<String, _>("kind").map_err(database_error)?,
                            &row.try_get::<String, _>("value").map_err(database_error)?,
                        )?,
                    },
                    lexical_ref: row.try_get("lexical_ref").map_err(database_error)?,
                    status: row.try_get("status").map_err(database_error)?,
                })
            })
            .collect()
    }

    /// Locate the Subject address before a consumer has selected a Subject.
    pub async fn subject_for_lexical(&self, lexical: &str) -> Result<SubjectId> {
        if validate_lexical(lexical)? != "sub" {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::ReferenceTypeMismatch,
                "Expected a Subject lexical reference",
            )
            .with_context("reference", lexical)
            .with_context("expected_kind", "subject")
            .into());
        }
        let canonical: String = sqlx::query_scalar("SELECT b.canonical_ref FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE b.object_kind='subject' AND b.lexical_ref=$1 AND b.tombstoned_at IS NULL AND v.subject_id::text=b.canonical_ref")
            .bind(lexical).fetch_optional(self.pool()).await.map_err(database_error)?
            .ok_or_else(|| nous_core::DomainError::new(nous_core::DomainErrorCode::UnknownReference, "Subject reference is unknown or unavailable")
                .with_context("reference", lexical))?;
        canonical
            .parse()
            .map(SubjectId)
            .map_err(|_| Error::Infrastructure("invalid Subject address".into()))
    }

    pub async fn bind_identity(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        display_name: String,
        aliases: Vec<String>,
    ) -> Result<IdentityBinding> {
        self.bind_identity_mode(subject, reference, display_name, aliases, false)
            .await
    }
    pub async fn ensure_identity_address(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        display_name: String,
    ) -> Result<IdentityBinding> {
        self.bind_identity_mode(subject, reference, display_name, vec![], true)
            .await
    }
    async fn bind_identity_mode(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        display_name: String,
        aliases: Vec<String>,
        address_only: bool,
    ) -> Result<IdentityBinding> {
        self.require_subject(subject).await?;
        let mut tx = self.begin().await?;
        let binding = self
            .bind_identity_in_mode(
                &mut tx,
                subject,
                reference,
                display_name,
                aliases,
                address_only,
            )
            .await?;
        tx.commit().await.map_err(database_error)?;
        let kind = reference_parts(&binding.canonical).0;
        let lexical = binding.lexical_ref;
        let (status, mut candidates) = self
            .resolve_identity(subject, &kind, &lexical, true)
            .await?;
        if status != "BOUND" {
            return Err(Error::NotFound(status));
        }
        candidates
            .pop()
            .ok_or_else(|| Error::Infrastructure("committed binding missing".into()))
    }
    /// Bind inside the semantic owner's Authority transaction.
    pub async fn bind_identity_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: CognitiveRef,
        display_name: String,
        aliases: Vec<String>,
    ) -> Result<IdentityBinding> {
        self.bind_identity_in_mode(tx, subject, reference, display_name, aliases, false)
            .await
    }
    async fn bind_identity_in_mode(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        mut reference: CognitiveRef,
        display_name: String,
        mut aliases: Vec<String>,
        address_only: bool,
    ) -> Result<IdentityBinding> {
        if let CognitiveRef::Tag(tag) = reference {
            reference = crate::tags::canonical_topology_ref_in(tx, subject, CognitiveRef::Tag(tag))
                .await?
                .ok_or_else(|| Error::NotFound("active Tag identity not found".into()))?;
        }
        if display_name.len() > 256
            || aliases.len() > 32
            || aliases.iter().any(|a| a.is_empty() || a.len() > 256)
        {
            return Err(Error::Invalid("invalid identity label bounds".into()));
        }
        if !matches!(reference, CognitiveRef::Entity(_))
            && !self
                .reference_in_subject_tx(tx, subject, &reference)
                .await?
        {
            return Err(Error::NotFound("cognitive reference not found".into()));
        }
        let (kind, canonical) = reference_parts(&reference);
        let prefix = lexical_prefix(&kind)?;
        aliases.sort();
        aliases.dedup();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,1))")
            .bind(format!("{kind}:{canonical}"))
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
        let row=sqlx::query("SELECT lexical_ref,tombstoned_at IS NOT NULL AS tombstoned FROM lexical_bindings WHERE object_kind=$1 AND canonical_ref=$2")
            .bind(&kind).bind(&canonical).fetch_optional(&mut **tx).await.map_err(database_error)?;
        let lexical = if let Some(row) = row {
            if row
                .try_get::<bool, _>("tombstoned")
                .map_err(database_error)?
            {
                return Err(nous_core::DomainError::new(
                    nous_core::DomainErrorCode::ReferenceTombstoned,
                    "The identity has been tombstoned",
                )
                .with_context("reference", format!("{kind}:{canonical}"))
                .into());
            }
            row.try_get::<String, _>("lexical_ref")
                .map_err(database_error)?
        } else {
            let mut inserted = None;
            for _ in 0..32 {
                let mut bytes = [0; 6];
                getrandom::fill(&mut bytes).map_err(|e| Error::Infrastructure(e.to_string()))?;
                let bits = u64::from_be_bytes([
                    0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
                ]);
                let candidate = format!("{prefix}:{}", encode_proquint(bits)?);
                if sqlx::query("INSERT INTO lexical_bindings(lexical_ref,object_kind,canonical_ref) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                    .bind(&candidate).bind(&kind).bind(&canonical).execute(&mut **tx).await.map_err(database_error)?.rows_affected()==1 {inserted=Some(candidate);break;}
            }
            inserted.ok_or_else(|| {
                Error::Infrastructure("LexicalRef collision retry bound exceeded".into())
            })?
        };
        sqlx::query("INSERT INTO lexical_visibility(subject_id,lexical_ref,display_name,aliases) VALUES($1,$2,$3,$4) ON CONFLICT(subject_id,lexical_ref) DO UPDATE SET display_name=CASE WHEN excluded.display_name='' THEN lexical_visibility.display_name ELSE excluded.display_name END,aliases=CASE WHEN cardinality(excluded.aliases)=0 THEN lexical_visibility.aliases ELSE excluded.aliases END WHERE NOT $5")
            .bind(subject.0).bind(&lexical).bind(&display_name).bind(&aliases).bind(address_only).execute(&mut **tx).await.map_err(database_error)?;
        Ok(IdentityBinding {
            lexical_ref: lexical,
            canonical: reference,
            display_name,
            aliases,
            status: "LIVE".into(),
        })
    }
    pub async fn resolve_identity(
        &self,
        subject: SubjectId,
        kind: &str,
        locator: &str,
        lexical: bool,
    ) -> Result<(String, Vec<IdentityBinding>)> {
        self.require_subject(subject).await?;
        if lexical {
            let prefix = validate_lexical(locator)?;
            if !kind.is_empty() && lexical_prefix(kind)? != prefix {
                return Ok(("REFERENCE_TYPE_MISMATCH".into(), vec![]));
            }
        } else if locator.is_empty() || locator.len() > 256 {
            return Err(Error::Invalid("invalid name locator".into()));
        }
        let rows = sqlx::query(r#"
WITH matched AS (
 SELECT b.lexical_ref,b.object_kind,
   CASE WHEN b.object_kind='tag' THEN canonical_tag($1,b.canonical_ref::uuid)::text ELSE b.canonical_ref END AS canonical_ref,
   b.tombstoned_at IS NOT NULL AS tombstoned,
   CASE WHEN b.object_kind='tag' THEN COALESCE((SELECT r.label FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.tag_id=canonical_tag($1,b.canonical_ref::uuid)),v.display_name) ELSE v.display_name END AS display_name,
   v.aliases
 FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref)
 WHERE v.subject_id=$1 AND ($2='' OR b.object_kind=$2)
 AND (($4 AND b.lexical_ref=$3) OR (NOT $4 AND (v.display_name=$3 OR $3=ANY(v.aliases))))
)
SELECT DISTINCT ON(object_kind,canonical_ref) * FROM matched ORDER BY object_kind,canonical_ref,lexical_ref LIMIT 65
"#).bind(subject.0).bind(kind).bind(locator).bind(lexical).fetch_all(self.pool()).await.map_err(database_error)?;
        if rows.len() > 64 {
            return Err(Error::Invalid(
                "identity ambiguity exceeds 64 candidates".into(),
            ));
        }
        let mut bindings = Vec::new();
        let mut tombstoned = false;
        for row in rows {
            if row
                .try_get::<bool, _>("tombstoned")
                .map_err(database_error)?
            {
                tombstoned = true;
                continue;
            }
            let Some(canonical) = row
                .try_get::<Option<String>, _>("canonical_ref")
                .map_err(database_error)?
            else {
                tombstoned = true;
                continue;
            };
            let reference = parse_reference(
                &row.try_get::<String, _>("object_kind")
                    .map_err(database_error)?,
                &canonical,
            )?;
            if !matches!(reference, CognitiveRef::Entity(_))
                && !self.reference_in_subject(subject, &reference).await?
            {
                tombstoned = true;
                continue;
            }
            bindings.push(IdentityBinding {
                lexical_ref: row.try_get("lexical_ref").map_err(database_error)?,
                canonical: reference,
                display_name: row.try_get("display_name").map_err(database_error)?,
                aliases: row.try_get("aliases").map_err(database_error)?,
                status: "LIVE".into(),
            });
        }
        let status = match bindings.len() {
            1 => "BOUND",
            2.. => "AMBIGUOUS_REFERENCE",
            _ if tombstoned => "REFERENCE_TOMBSTONED",
            _ => "UNKNOWN_REFERENCE",
        };
        Ok((status.into(), bindings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_reference_validation() {
        assert_eq!(validate_lexical("mem:bahog-hijol-mokor").unwrap(), "mem");
        assert!(validate_lexical("Mem:bahog-hijol-mokor").is_err());
        assert!(validate_lexical("unknown:bahog-hijol-mokor").is_err());
        assert!(validate_lexical("mem:bahog-hijol").is_err());
    }
}
