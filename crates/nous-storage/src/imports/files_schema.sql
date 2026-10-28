CREATE TABLE IF NOT EXISTS import_file_successes (
 id TEXT PRIMARY KEY NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),
 namespace_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.namespace_id')) STORED NOT NULL,
 subject_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.subject_id')) STORED NOT NULL,
 input_source_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.input_source_id')) STORED NOT NULL,
 catalog_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.catalog_id')) STORED NOT NULL,
 commit_seq INTEGER GENERATED ALWAYS AS(json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
 CHECK(json_extract(body,'$.id')=id),
 FOREIGN KEY(namespace_id,subject_id,input_source_id) REFERENCES import_input_sources(namespace_id,subject_id,id),
 FOREIGN KEY(namespace_id,subject_id,catalog_id) REFERENCES import_unit_catalogs(namespace_id,subject_id,id),
 UNIQUE(namespace_id,subject_id,id)
);
CREATE INDEX IF NOT EXISTS file_successes_by_subject ON import_file_successes(namespace_id,subject_id,commit_seq,id);
CREATE INDEX IF NOT EXISTS file_successes_by_input ON import_file_successes(namespace_id,subject_id,input_source_id,catalog_id);
CREATE TABLE IF NOT EXISTS import_manifest_heads (
 namespace_id TEXT NOT NULL,
 subject_id TEXT NOT NULL,
 origin_key TEXT NOT NULL CHECK(json_valid(origin_key)),
 policy TEXT NOT NULL CHECK(policy IN('content_hash','manifest')),
 manifest_key TEXT NOT NULL CHECK(json_valid(manifest_key)),
 body TEXT NOT NULL CHECK(json_valid(body)),
 success_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.success_id')) STORED NOT NULL,
 commit_seq INTEGER GENERATED ALWAYS AS(json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
 CHECK(json_extract(body,'$.namespace_id')=namespace_id AND json_extract(body,'$.subject_id')=subject_id),
 CHECK(coalesce(json_extract(body,'$.origin'),'null')=origin_key),
 CHECK(json_extract(body,'$.key')=manifest_key AND json_extract(body,'$.policy')=policy),
 PRIMARY KEY(namespace_id,subject_id,origin_key,policy,manifest_key),
 FOREIGN KEY(namespace_id,subject_id,success_id) REFERENCES import_file_successes(namespace_id,subject_id,id)
);
