CREATE TABLE IF NOT EXISTS import_unit_outputs (
 id TEXT PRIMARY KEY NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),
 namespace_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.namespace_id')) STORED NOT NULL,
 subject_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.subject_id')) STORED NOT NULL,
 unit_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.unit_id')) STORED NOT NULL,
 commit_seq INTEGER GENERATED ALWAYS AS (json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
 CHECK(json_extract(body,'$.id')=id),
 FOREIGN KEY(unit_id) REFERENCES import_text_units(id)
);
CREATE INDEX IF NOT EXISTS import_outputs_by_subject ON import_unit_outputs(namespace_id,subject_id,commit_seq,id);
