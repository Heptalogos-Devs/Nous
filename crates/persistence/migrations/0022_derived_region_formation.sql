-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- A region's formation is its own segmentation, not its parent's formation.
ALTER TABLE derived_regions ADD COLUMN formed_stamp jsonb;
UPDATE derived_regions SET formed_stamp=jsonb_build_object('time_domain_ref','time:real','at',created_at,'recorded_at',created_at);
ALTER TABLE derived_regions ALTER COLUMN formed_stamp SET NOT NULL;
