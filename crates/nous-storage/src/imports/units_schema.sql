CREATE TABLE IF NOT EXISTS import_unit_catalogs (
    id TEXT PRIMARY KEY NOT NULL,
    body TEXT NOT NULL CHECK(json_valid(body)),
    namespace_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.namespace_id')) STORED NOT NULL,
    subject_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.subject_id')) STORED NOT NULL,
    parent_catalog_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.parent_catalog_id')) STORED,
    commit_seq INTEGER GENERATED ALWAYS AS(json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
    CHECK(json_extract(body,'$.id')=id),
    FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
    FOREIGN KEY(namespace_id,subject_id,parent_catalog_id) REFERENCES import_unit_catalogs(namespace_id,subject_id,id),
    UNIQUE(namespace_id,subject_id,id)
);
CREATE TABLE IF NOT EXISTS import_text_units (
    id TEXT PRIMARY KEY NOT NULL,
    body TEXT NOT NULL CHECK(json_valid(body)),
    namespace_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.namespace_id')) STORED NOT NULL,
    subject_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.subject_id')) STORED NOT NULL,
    catalog_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.catalog_id')) STORED NOT NULL,
    input_source_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.input_source_id')) STORED NOT NULL,
    source_region_id TEXT GENERATED ALWAYS AS(json_extract(body,'$.source_region_id')) STORED NOT NULL,
    ordinal INTEGER GENERATED ALWAYS AS(json_extract(body,'$.ordinal')) STORED NOT NULL CHECK(ordinal>=0),
    commit_seq INTEGER GENERATED ALWAYS AS(json_extract(body,'$.commit_seq')) STORED NOT NULL CHECK(commit_seq>0),
    CHECK(json_extract(body,'$.id')=id),
    FOREIGN KEY(namespace_id,subject_id,catalog_id) REFERENCES import_unit_catalogs(namespace_id,subject_id,id),
    FOREIGN KEY(namespace_id,subject_id,input_source_id) REFERENCES import_input_sources(namespace_id,subject_id,id),
    FOREIGN KEY(source_region_id) REFERENCES source_regions(id),
    UNIQUE(catalog_id,ordinal)
);
CREATE INDEX IF NOT EXISTS import_units_by_input ON import_text_units(namespace_id,subject_id,input_source_id,ordinal,id);
