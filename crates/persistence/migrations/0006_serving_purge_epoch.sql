-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

-- Serving generations and query bindings are fenced by the owner's current
-- purge epoch. Existing, non-purging subjects start at epoch zero.
ALTER TABLE subjects
    ADD COLUMN serving_purge_epoch bigint NOT NULL DEFAULT 0
    CHECK (serving_purge_epoch >= 0);

ALTER TABLE serving_generations
    ADD COLUMN purge_epoch bigint NOT NULL DEFAULT 0
    CHECK (purge_epoch >= 0);

-- `ready` is the current generation state. Keep one current artifact per
-- family/space within each purge epoch; prior epochs remain physically
-- reclaimable without blocking publication after a purge checkpoint.
CREATE UNIQUE INDEX serving_one_ready_current_generation
    ON serving_generations(subject_id, family, (COALESCE(space_signature, '')), purge_epoch)
    WHERE state = 'ready' AND view_descriptor->>'kind' = 'current';

-- Embeddings are content-addressed and shared by all canonical refs with the
-- same representation. This join records exact ownership without duplicating
-- the vector or its content identity.
CREATE TABLE embedding_material_uses (
    subject_id uuid NOT NULL,
    content_digest text NOT NULL,
    space_hash text NOT NULL,
    producer_hash text NOT NULL,
    ref_kind text NOT NULL,
    ref_value text NOT NULL,
    PRIMARY KEY (subject_id, content_digest, space_hash, producer_hash, ref_kind, ref_value),
    FOREIGN KEY (subject_id, content_digest, space_hash, producer_hash)
        REFERENCES embedding_materials(subject_id, content_digest, space_hash, producer_hash)
        ON DELETE CASCADE
);
CREATE INDEX embedding_material_uses_by_ref
    ON embedding_material_uses(subject_id, ref_kind, ref_value);
