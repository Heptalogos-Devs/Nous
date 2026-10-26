CREATE TABLE IF NOT EXISTS work_context_revisions (
 id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(id,revision),FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
CREATE INDEX IF NOT EXISTS work_contexts_scope ON work_context_revisions(namespace_id,subject_id,id,revision DESC);
CREATE TABLE IF NOT EXISTS context_sessions (
 id TEXT PRIMARY KEY NOT NULL,namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,parent_id TEXT,
 native_session TEXT NOT NULL,history_generation TEXT NOT NULL,active_work_context TEXT,
 revision INTEGER NOT NULL CHECK(revision>0),history_invalidated INTEGER NOT NULL CHECK(history_invalidated IN(0,1)),
 body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 UNIQUE(namespace_id,subject_id,native_session,history_generation),FOREIGN KEY(parent_id) REFERENCES context_sessions(id),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
CREATE TABLE IF NOT EXISTS context_preparations (
 id TEXT PRIMARY KEY NOT NULL,job_id TEXT NOT NULL UNIQUE REFERENCES task_records(job_id),request TEXT NOT NULL CHECK(json_valid(request)),
 config_digest TEXT,policy TEXT CHECK(policy IS NULL OR json_valid(policy)),catalog TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(catalog)),
 work_context TEXT CHECK(work_context IS NULL OR json_valid(work_context)),session_revision INTEGER,source_watermark INTEGER,
 steward_job_id TEXT REFERENCES task_records(job_id),response TEXT CHECK(response IS NULL OR json_valid(response)),
 degradation TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(degradation))
);
CREATE TABLE IF NOT EXISTS context_residents (
 session_id TEXT NOT NULL REFERENCES context_sessions(id),target TEXT NOT NULL CHECK(json_valid(target)),
 state TEXT NOT NULL CHECK(state IN('resident','provisional','evicted')),entry_reason TEXT NOT NULL,entered_at TEXT NOT NULL,
 last_meaningful_use_at TEXT,hold_until TEXT,PRIMARY KEY(session_id,target)
);
CREATE TABLE IF NOT EXISTS context_steward_jobs (
 job_id TEXT PRIMARY KEY NOT NULL REFERENCES task_records(job_id),preparation_id TEXT NOT NULL UNIQUE REFERENCES context_preparations(id)
);
CREATE TABLE IF NOT EXISTS context_projections (
 id TEXT PRIMARY KEY NOT NULL,namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,session_id TEXT NOT NULL REFERENCES context_sessions(id),
 preparation_id TEXT NOT NULL UNIQUE REFERENCES context_preparations(id),body TEXT NOT NULL CHECK(json_valid(body)),
 commit_seq INTEGER NOT NULL CHECK(commit_seq>0),FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
CREATE TABLE IF NOT EXISTS context_admissions (
 id TEXT PRIMARY KEY NOT NULL,projection_id TEXT NOT NULL REFERENCES context_projections(id),
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,session_id TEXT NOT NULL REFERENCES context_sessions(id),
 consumer_id TEXT NOT NULL,instance_id TEXT NOT NULL,invocation_id TEXT NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 UNIQUE(namespace_id,subject_id,consumer_id,instance_id,invocation_id),FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
CREATE TABLE IF NOT EXISTS context_outcomes (
 admission_id TEXT PRIMARY KEY NOT NULL REFERENCES context_admissions(id),body TEXT NOT NULL CHECK(json_valid(body)),
 recorded_at TEXT NOT NULL,commit_seq INTEGER NOT NULL CHECK(commit_seq>0)
);
CREATE TABLE IF NOT EXISTS context_use_events (
 namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,consumer_id TEXT NOT NULL,instance_id TEXT NOT NULL,event_id TEXT NOT NULL,
 admission_id TEXT NOT NULL REFERENCES context_admissions(id),target TEXT NOT NULL CHECK(json_valid(target)),kind TEXT NOT NULL,
 occurred_at TEXT NOT NULL,recorded_at TEXT NOT NULL,request_hash TEXT NOT NULL,commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 PRIMARY KEY(namespace_id,subject_id,consumer_id,instance_id,event_id),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id)
);
