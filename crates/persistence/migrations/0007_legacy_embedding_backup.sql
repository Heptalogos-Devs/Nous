-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

-- Existing vector rows without a reconstructed cognitive reference cannot stay
-- in the active cache. Keep them in a separately managed backup, outside owner
-- cascades and purge deletion scope, until an explicit backup lifecycle acts.
CREATE TABLE legacy_embedding_materials_backup (
    subject_id uuid NOT NULL,
    content_digest text NOT NULL,
    space_hash text NOT NULL,
    producer_hash text NOT NULL,
    vector real[] NOT NULL,
    source_created_at timestamptz NOT NULL,
    backed_up_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (subject_id, content_digest, space_hash, producer_hash)
);

COMMENT ON TABLE legacy_embedding_materials_backup IS
    'Legacy vectors whose cognitive source could not be reconstructed. This is not an active cache and is outside semantic-owner purge scope; remove only through the explicit backup lifecycle.';
