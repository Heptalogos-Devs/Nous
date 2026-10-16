//! Durable single-world Subject clock. Infrastructure clocks never use this mapping.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::CognitiveClock;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use nous_core::{DomainStamp, Error, OperationId, Result, SubjectId};
use nous_persistence::{
    AuthorityStore, MutationEnvelope, MutationStart, OwnerLock, database_error as db,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

pub const REAL_TIME_DOMAIN: &str = "time:real";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockSnapshot {
    pub subject_id: SubjectId,
    pub time_domain_ref: String,
    pub world_anchor: DateTime<Utc>,
    pub real_anchor: DateTime<Utc>,
    pub rate: f64,
    pub world_now: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClockChange {
    Configure {
        time_domain_ref: String,
        rate: f64,
        world_time: Option<DateTime<Utc>>,
    },
    Pause,
    Advance {
        seconds: f64,
    },
    Resume {
        rate: f64,
    },
}

struct RunningClock {
    snapshot: ClockSnapshot,
    monotonic_anchor: Instant,
    high_water: DateTime<Utc>,
}
impl RunningClock {
    fn now(&mut self) -> DateTime<Utc> {
        let seconds = self.monotonic_anchor.elapsed().as_secs_f64() * self.snapshot.rate;
        let elapsed = Duration::microseconds((seconds * 1_000_000.0).min(i64::MAX as f64) as i64);
        let candidate = self
            .snapshot
            .world_anchor
            .checked_add_signed(elapsed)
            .unwrap_or(DateTime::<Utc>::MAX_UTC.trunc_subsecs(6));
        self.high_water = self.high_water.max(candidate);
        self.high_water
    }
}

/// One kernel owns the in-process mapping; durable checkpoints share domain commits.
#[derive(Clone)]
pub struct SubjectClockService {
    store: AuthorityStore,
    clocks: Arc<Mutex<HashMap<SubjectId, RunningClock>>>,
    fallback: Arc<dyn CognitiveClock>,
    changes: Arc<tokio::sync::Mutex<()>>,
}
impl SubjectClockService {
    pub async fn open(store: AuthorityStore, fallback: Arc<dyn CognitiveClock>) -> Result<Self> {
        let service = Self {
            store,
            clocks: Default::default(),
            fallback,
            changes: Default::default(),
        };
        let rows = sqlx::query("SELECT * FROM subject_clocks")
            .fetch_all(service.store.pool())
            .await
            .map_err(db)?;
        let real = Utc::now().trunc_subsecs(6);
        for row in rows {
            let subject = SubjectId(row.try_get("subject_id").map_err(db)?);
            let checkpoint: DateTime<Utc> = row.try_get("checkpoint_world").map_err(db)?;
            let checkpoint_real: DateTime<Utc> = row.try_get("checkpoint_real").map_err(db)?;
            let rate: f64 = row.try_get("rate").map_err(db)?;
            // Offline time progresses at real speed, unless explicitly paused.
            let offline = if rate == 0.0 {
                Duration::zero()
            } else {
                (real - checkpoint_real).max(Duration::zero())
            };
            let world = checkpoint
                .checked_add_signed(offline)
                .ok_or_else(|| Error::Invalid("clock timestamp overflow".into()))?;
            let snapshot = ClockSnapshot {
                subject_id: subject,
                time_domain_ref: row.try_get("time_domain_ref").map_err(db)?,
                world_anchor: world,
                real_anchor: real,
                rate,
                world_now: world,
                recorded_at: real,
                revision: row.try_get("revision").map_err(db)?,
            };
            service
                .clocks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    subject,
                    RunningClock {
                        snapshot,
                        monotonic_anchor: Instant::now(),
                        high_water: world,
                    },
                );
        }
        Ok(service)
    }
    fn snapshot(&self, subject: SubjectId) -> ClockSnapshot {
        let mut clocks = self.clocks.lock().unwrap_or_else(|e| e.into_inner());
        let entry = clocks.entry(subject).or_insert_with(|| {
            let world = self.fallback.now(subject).trunc_subsecs(6);
            RunningClock {
                snapshot: ClockSnapshot {
                    subject_id: subject,
                    time_domain_ref: self.fallback.time_domain_ref(subject),
                    world_anchor: world,
                    real_anchor: Utc::now(),
                    rate: if self.fallback.time_domain_ref(subject) == REAL_TIME_DOMAIN {
                        1.0
                    } else {
                        0.0
                    },
                    world_now: world,
                    recorded_at: Utc::now(),
                    revision: 0,
                },
                monotonic_anchor: Instant::now(),
                high_water: world,
            }
        });
        if entry.snapshot.revision == 0
            && self.fallback.time_domain_ref(subject) != REAL_TIME_DOMAIN
        {
            entry.high_water = entry
                .high_water
                .max(self.fallback.now(subject).trunc_subsecs(6));
            entry.snapshot.world_anchor = entry.high_water;
        }
        let world = entry.now();
        ClockSnapshot {
            world_now: world,
            recorded_at: Utc::now(),
            ..entry.snapshot.clone()
        }
    }
    /// Domain-aware owners capture this stamp inside the transaction that commits
    /// their semantic result. No checkpoint survives a rolled-back owner write.
    pub async fn capture_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
    ) -> Result<DomainStamp> {
        // Capture once per owner transaction, before other Subject clock work.
        let _change = self.changes.lock().await;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("subject-clock:{}", subject.0))
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        let snapshot = self.snapshot(subject);
        sqlx::query("INSERT INTO subject_clocks(subject_id,time_domain_ref,rate,checkpoint_world,checkpoint_real,revision) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(subject_id) DO UPDATE SET checkpoint_world=GREATEST(subject_clocks.checkpoint_world,excluded.checkpoint_world),checkpoint_real=GREATEST(subject_clocks.checkpoint_real,excluded.checkpoint_real)")
            .bind(subject.0).bind(&snapshot.time_domain_ref).bind(snapshot.rate).bind(snapshot.world_now).bind(snapshot.recorded_at).bind(snapshot.revision).execute(&mut **tx).await.map_err(db)?;
        Ok(DomainStamp {
            time_domain_ref: snapshot.time_domain_ref,
            at: snapshot.world_now,
            recorded_at: snapshot.recorded_at,
        })
    }
    pub async fn read(&self, subject: SubjectId) -> Result<ClockSnapshot> {
        let _change = self.changes.lock().await;
        self.store.require_subject(subject).await?;
        let snapshot = self.snapshot(subject);
        self.persist_checkpoint(&snapshot).await?;
        Ok(snapshot)
    }
    async fn persist_checkpoint(&self, snapshot: &ClockSnapshot) -> Result<()> {
        sqlx::query("INSERT INTO subject_clocks(subject_id,time_domain_ref,rate,checkpoint_world,checkpoint_real,revision) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(subject_id) DO UPDATE SET checkpoint_world=GREATEST(subject_clocks.checkpoint_world,excluded.checkpoint_world),checkpoint_real=CASE WHEN excluded.checkpoint_world>=subject_clocks.checkpoint_world THEN GREATEST(subject_clocks.checkpoint_real,excluded.checkpoint_real) ELSE subject_clocks.checkpoint_real END")
            .bind(snapshot.subject_id.0).bind(&snapshot.time_domain_ref).bind(snapshot.rate).bind(snapshot.world_now).bind(snapshot.recorded_at).bind(snapshot.revision).execute(self.store.pool()).await.map_err(db)?;
        Ok(())
    }
    pub async fn change(
        &self,
        subject: SubjectId,
        operation: OperationId,
        change: ClockChange,
    ) -> Result<ClockSnapshot> {
        if operation.0.is_nil() {
            return Err(Error::Invalid("operation_id is required".into()));
        }
        let _change = self.changes.lock().await;
        self.store.require_subject(subject).await?;
        let digest = nous_core::canonical_request_digest("clock.change", subject, &change)?;
        let mut mutation = match MutationEnvelope::begin(
            &self.store,
            subject,
            operation,
            "clock.change",
            &digest,
            Some(OwnerLock("subject-clock")),
        )
        .await?
        {
            MutationStart::Replay(receipt) => {
                let _ = receipt;
                let saved: serde_json::Value = sqlx::query_scalar("SELECT snapshot FROM subject_clock_changes WHERE subject_id=$1 AND operation_id=$2").bind(subject.0).bind(operation.0).fetch_one(self.store.pool()).await.map_err(db)?;
                return serde_json::from_value(saved)
                    .map_err(|e| Error::Infrastructure(e.to_string()));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let mut snapshot = self.snapshot(subject);
        let previous = snapshot.world_now;
        match change {
            ClockChange::Configure {
                time_domain_ref,
                rate,
                world_time,
            } => {
                if time_domain_ref.trim().is_empty() || time_domain_ref.len() > 256 {
                    return Err(Error::Invalid(
                        "time_domain_ref must contain 1..256 bytes".into(),
                    ));
                }
                if snapshot.time_domain_ref != REAL_TIME_DOMAIN
                    && snapshot.time_domain_ref != time_domain_ref
                    && snapshot.revision != 0
                {
                    return Err(Error::Invalid("Subject has one primary time domain; use an isolated Subject for another world".into()));
                }
                snapshot.time_domain_ref = time_domain_ref;
                snapshot.rate = valid_rate(rate)?;
                snapshot.world_now = world_time.unwrap_or(previous).trunc_subsecs(6);
            }
            ClockChange::Pause => snapshot.rate = 0.0,
            ClockChange::Advance { seconds } => {
                valid_rate(seconds)?;
                let micros = seconds * 1_000_000.0;
                if micros > i64::MAX as f64 {
                    return Err(Error::Invalid(
                        "clock advance exceeds duration range".into(),
                    ));
                }
                snapshot.world_now = previous
                    .checked_add_signed(Duration::microseconds(micros as i64))
                    .ok_or_else(|| Error::Invalid("clock timestamp overflow".into()))?;
            }
            ClockChange::Resume { rate } => {
                snapshot.rate = valid_rate(rate)?;
                if rate == 0.0 {
                    return Err(Error::Invalid("resume rate must be positive".into()));
                }
            }
        }
        if snapshot.world_now < previous {
            return Err(Error::Invalid(
                "CognitiveClock cannot move backwards".into(),
            ));
        }
        if snapshot.time_domain_ref == REAL_TIME_DOMAIN
            && (snapshot.rate != 1.0 || snapshot.world_now > previous)
        {
            snapshot.time_domain_ref = format!("time:subject:{}", subject.0);
        }
        snapshot.real_anchor = Utc::now();
        snapshot.world_anchor = snapshot.world_now;
        snapshot.recorded_at = snapshot.real_anchor;
        snapshot.revision += 1;
        sqlx::query("INSERT INTO subject_clocks(subject_id,time_domain_ref,rate,checkpoint_world,checkpoint_real,revision) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(subject_id) DO UPDATE SET time_domain_ref=excluded.time_domain_ref,rate=excluded.rate,checkpoint_world=GREATEST(subject_clocks.checkpoint_world,excluded.checkpoint_world),checkpoint_real=excluded.checkpoint_real,revision=excluded.revision")
            .bind(subject.0).bind(&snapshot.time_domain_ref).bind(snapshot.rate).bind(snapshot.world_now).bind(snapshot.real_anchor).bind(snapshot.revision).execute(&mut **mutation.tx()).await.map_err(db)?;
        sqlx::query(
            "INSERT INTO subject_clock_changes(subject_id,operation_id,snapshot) VALUES($1,$2,$3)",
        )
        .bind(subject.0)
        .bind(operation.0)
        .bind(serde_json::to_value(&snapshot).map_err(|e| Error::Infrastructure(e.to_string()))?)
        .execute(&mut **mutation.tx())
        .await
        .map_err(db)?;
        mutation
            .commit(
                "subject-clock",
                Some(&subject.0.to_string()),
                None,
                Some(snapshot.revision),
            )
            .await?;
        self.clocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                subject,
                RunningClock {
                    high_water: snapshot.world_now,
                    snapshot: snapshot.clone(),
                    monotonic_anchor: Instant::now(),
                },
            );
        Ok(snapshot)
    }
}
#[async_trait::async_trait]
impl CognitiveClock for SubjectClockService {
    fn time_domain_ref(&self, subject: SubjectId) -> String {
        self.snapshot(subject).time_domain_ref
    }
    async fn read_stamp(&self, subject: SubjectId) -> Result<DomainStamp> {
        let s = self.read(subject).await?;
        Ok(DomainStamp {
            time_domain_ref: s.time_domain_ref,
            at: s.world_now,
            recorded_at: s.recorded_at,
        })
    }
    async fn capture_in(
        &self,
        tx: &mut nous_persistence::AuthorityTransaction<'_>,
        subject: SubjectId,
    ) -> Result<DomainStamp> {
        SubjectClockService::capture_in(self, tx, subject).await
    }
    fn real_deadline(
        &self,
        subject: SubjectId,
        domain: &str,
        at: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        if domain == REAL_TIME_DOMAIN {
            return Some(at);
        }
        let current = self.snapshot(subject);
        if domain != current.time_domain_ref {
            return None;
        }
        let now = Utc::now();
        if at <= current.world_now {
            return Some(now);
        }
        if current.rate == 0.0 {
            return None;
        }
        let micros = (at - current.world_now).num_microseconds()? as f64 / current.rate;
        if !micros.is_finite() || micros > i64::MAX as f64 {
            return None;
        }
        now.checked_add_signed(Duration::microseconds(micros.ceil() as i64))
    }
    fn now(&self, subject: SubjectId) -> DateTime<Utc> {
        self.snapshot(subject).world_now
    }
}
fn valid_rate(rate: f64) -> Result<f64> {
    if !rate.is_finite() || rate < 0.0 {
        return Err(Error::Invalid(
            "clock rate must be finite and nonnegative".into(),
        ));
    }
    Ok(rate)
}
