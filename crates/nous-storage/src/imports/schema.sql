CREATE TABLE IF NOT EXISTS import_input_sources (
    id TEXT PRIMARY KEY NOT NULL,
    body TEXT NOT NULL CHECK(json_valid(body)),
    namespace_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.namespace_id')) STORED NOT NULL,
    subject_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.subject_id')) STORED NOT NULL,
    material_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.material_id')) STORED NOT NULL,
    commit_seq INTEGER GENERATED ALWAYS AS (json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
    CHECK(json_extract(body,'$.id')=id),
    FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
    FOREIGN KEY(namespace_id,material_id) REFERENCES materials(namespace_id,id),
    UNIQUE(namespace_id,subject_id,id)
);
CREATE INDEX IF NOT EXISTS import_inputs_by_subject ON import_input_sources(namespace_id,subject_id,commit_seq,id);
CREATE TABLE IF NOT EXISTS import_record_containers (
    id TEXT PRIMARY KEY NOT NULL,
    body TEXT NOT NULL CHECK(json_valid(body)),
    namespace_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.namespace_id')) STORED NOT NULL,
    subject_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.subject_id')) STORED NOT NULL,
    input_source_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.input_source_id')) STORED NOT NULL,
    record_material_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.record_material_id')) STORED NOT NULL,
    observation_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.observation_id')) STORED NOT NULL,
    memory_id TEXT GENERATED ALWAYS AS (json_extract(body,'$.memory_id')) STORED NOT NULL,
    commit_seq INTEGER GENERATED ALWAYS AS (json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
    CHECK(json_extract(body,'$.id')=id),
    FOREIGN KEY(namespace_id,subject_id,input_source_id) REFERENCES import_input_sources(namespace_id,subject_id,id),
    FOREIGN KEY(namespace_id,record_material_id) REFERENCES materials(namespace_id,id),
    FOREIGN KEY(namespace_id,subject_id,observation_id,record_material_id) REFERENCES observations(namespace_id,subject_id,id,material_id)
);
CREATE INDEX IF NOT EXISTS import_containers_by_record ON import_record_containers(namespace_id,subject_id,record_material_id,commit_seq,id);
