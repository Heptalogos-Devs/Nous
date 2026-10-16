-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
CREATE TABLE subject_reflection_progress (
 subject_id uuid PRIMARY KEY REFERENCES subjects(subject_id) ON DELETE CASCADE,
 last_recorded_seq bigint NOT NULL DEFAULT 0 CHECK(last_recorded_seq>=0),
 last_operation_key text,
 last_status text,
 last_problem_code text,
 updated_at timestamptz NOT NULL DEFAULT now()
);
-- Paid reflection results outlive the short-lived maintenance need. Purge still
-- uses exact dependencies through the existing model-results owner.
ALTER TABLE model_results DROP CONSTRAINT model_results_maintenance_need_id_fkey;
ALTER TABLE model_results ADD CONSTRAINT model_results_maintenance_need_id_fkey FOREIGN KEY(maintenance_need_id) REFERENCES maintenance_needs(need_id) ON DELETE SET NULL;
ALTER TABLE subject_reflection_progress ADD COLUMN last_change_id bigint NOT NULL DEFAULT 0;
CREATE TABLE subject_reflection_source_changes (
 change_id bigserial PRIMARY KEY,
 subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
 reference jsonb NOT NULL,
 reason text NOT NULL,
 authority_sequence bigint NOT NULL,
 recorded_at timestamptz NOT NULL,
 UNIQUE(subject_id,reference,reason,authority_sequence)
);
CREATE INDEX subject_reflection_changes_cursor ON subject_reflection_source_changes(subject_id,change_id);
