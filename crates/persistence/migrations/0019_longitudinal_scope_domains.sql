-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
ALTER TABLE episode_revisions ADD COLUMN experience_time_domain_ref text NOT NULL DEFAULT 'time:real';
ALTER TABLE journal_revisions ADD COLUMN temporal_scope_domain_ref text NOT NULL DEFAULT 'time:real';
