-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Capture time of formation is distinct from time this authority write commits.
CREATE FUNCTION authority_formation_recorded_at() RETURNS timestamptz LANGUAGE SQL STABLE AS $$
 SELECT COALESCE((NULLIF(current_setting('nous.formed_stamp',true),'')::jsonb->>'recorded_at')::timestamptz,authority_recording_time())
$$;
DO $$
DECLARE owner_table text;
BEGIN
 FOREACH owner_table IN ARRAY ARRAY['memory_revisions','cognitive_schema_revisions','episode_revisions','journal_revisions','self_revisions','social_revisions'] LOOP
  EXECUTE format('ALTER TABLE %I ADD COLUMN formed_recorded_at timestamptz',owner_table);
  EXECUTE format('UPDATE %I SET formed_recorded_at=recorded_at',owner_table);
  EXECUTE format('ALTER TABLE %I ALTER COLUMN formed_recorded_at SET NOT NULL, ALTER COLUMN formed_recorded_at SET DEFAULT authority_formation_recorded_at()',owner_table);
 END LOOP;
END $$;
