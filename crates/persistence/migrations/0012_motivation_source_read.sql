-- Motivation extends the single fixed SourceRead dispatcher after both owners exist.
CREATE OR REPLACE FUNCTION source_read_edges_with_mode(owner_subject uuid,source_kind text,source_value text,metadata_only boolean)
RETURNS TABLE(kind text,value text,metadata_edge boolean) LANGUAGE plpgsql STABLE AS $$ BEGIN
 IF source_kind IN ('desire','desire_revision','concern','concern_revision','desired_condition','desired_condition_revision') THEN
 RETURN QUERY SELECT e.kind,e.value,e.metadata_edge FROM motivation_source_read_edges_with_mode(owner_subject,source_kind,source_value,metadata_only) e;
 ELSE RETURN QUERY SELECT e.kind,e.value,false FROM source_read_edges(owner_subject,source_kind,source_value,metadata_only) e;
 END IF;
END $$;
