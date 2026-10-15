-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

-- Session lineage belongs to Runtime; it carries no copied cognitive content.
ALTER TABLE cognitive_sessions
 ADD COLUMN parent_session_id uuid,
 ADD COLUMN history_invalidated boolean NOT NULL DEFAULT false,
 ADD COLUMN history_domains text[] NOT NULL DEFAULT '{}',
 ADD CONSTRAINT cognitive_sessions_subject_identity UNIQUE(subject_id,session_id),
 ADD CONSTRAINT cognitive_sessions_parent_subject_fk FOREIGN KEY(subject_id,parent_session_id)
  REFERENCES cognitive_sessions(subject_id,session_id);
CREATE INDEX cognitive_sessions_parent ON cognitive_sessions(parent_session_id);

-- Preserve the meaning of already-recorded uses when upgrading an existing instance.
UPDATE cognitive_sessions s SET history_domains=ARRAY(
 SELECT DISTINCT CASE WHEN e.ref_kind IN ('self_facet_revision','narrative_thread_revision') THEN 'self' ELSE 'memory' END
 FROM cognitive_use_events e WHERE e.subject_id=s.subject_id AND e.session_id=s.session_id
);
-- Old purge receipts intentionally have no retained Session/source content.
-- Such old histories cannot be certified clean, so require a native reset once.
UPDATE cognitive_sessions s SET history_invalidated=true,runtime_revision=runtime_revision+1
 WHERE EXISTS(SELECT 1 FROM purged_use_receipts p WHERE p.subject_id=s.subject_id);
