-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0

-- One primary world per Subject. Real recording time remains separate from semantic time.
CREATE TABLE subject_clocks (
    subject_id uuid PRIMARY KEY REFERENCES subjects(subject_id) ON DELETE CASCADE,
    time_domain_ref text NOT NULL CHECK(length(time_domain_ref) BETWEEN 1 AND 256),
    rate double precision NOT NULL CHECK(rate >= 0 AND rate < 'Infinity'::float8),
    checkpoint_world timestamptz NOT NULL,
    checkpoint_real timestamptz NOT NULL,
    revision bigint NOT NULL DEFAULT 0 CHECK(revision >= 0)
);

CREATE TABLE subject_clock_changes (
    subject_id uuid NOT NULL REFERENCES subjects(subject_id) ON DELETE CASCADE,
    operation_id uuid NOT NULL,
    snapshot jsonb NOT NULL,
    PRIMARY KEY(subject_id,operation_id)
);
