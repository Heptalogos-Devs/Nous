-- Copyright 2026 Aravine Zhu
-- SPDX-License-Identifier: Apache-2.0
-- Pure math for canonical JSON extents. No source, permission or coverage claims.
-- Strip fractional seconds before the PostgreSQL cast, then retain all nine
-- canonical nanosecond digits rather than rounding a point across a boundary.
CREATE FUNCTION temporal_json_coordinate(value text) RETURNS numeric
LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT extract(epoch FROM regexp_replace(value,'\.[0-9]+Z$','Z')::timestamptz)*1000000000
  + rpad(COALESCE((regexp_match(value,'\.([0-9]+)Z$'))[1],''),9,'0')::numeric
$$;
CREATE FUNCTION domain_temporal_matches(extent jsonb, domain text, predicate jsonb)
RETURNS boolean LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE lo numeric; hi numeric; point numeric; from_at numeric; to_at numeric;
BEGIN
 IF domain IS DISTINCT FROM predicate->>'time_domain_ref' OR extent->>'kind'='unknown' OR extent IS NULL THEN RETURN false; END IF;
 IF extent->>'kind'='instant' THEN
  point=temporal_json_coordinate(extent->>'at');
  IF predicate#>>'{predicate,kind}'='point' THEN RETURN point=temporal_json_coordinate(predicate#>>'{predicate,at}'); END IF;
  from_at=temporal_json_coordinate(predicate#>>'{predicate,start}');
  to_at=temporal_json_coordinate(predicate#>>'{predicate,end}');
  RETURN (from_at IS NULL OR point>=from_at) AND (to_at IS NULL OR point<to_at);
 END IF;
 IF extent->>'kind' IS DISTINCT FROM 'interval' THEN RETURN false; END IF;
 lo=temporal_json_coordinate(extent->>'start'); hi=temporal_json_coordinate(extent->>'end');
 IF predicate#>>'{predicate,kind}'='point' THEN
  point=temporal_json_coordinate(predicate#>>'{predicate,at}');
  RETURN (lo IS NULL OR point>=lo) AND (hi IS NULL OR point<hi);
 END IF;
 from_at=temporal_json_coordinate(predicate#>>'{predicate,start}'); to_at=temporal_json_coordinate(predicate#>>'{predicate,end}');
 RETURN (hi IS NULL OR from_at IS NULL OR from_at<hi) AND (lo IS NULL OR to_at IS NULL OR lo<to_at);
END $$;
