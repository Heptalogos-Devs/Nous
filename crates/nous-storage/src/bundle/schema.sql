CREATE TABLE IF NOT EXISTS bundle_installations (
 id TEXT PRIMARY KEY NOT NULL, namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 source_namespace_id TEXT NOT NULL,source_subject_id TEXT NOT NULL,
 package_json TEXT NOT NULL CHECK(json_valid(package_json) AND json_type(package_json)='object' AND json_type(package_json,'$.id') IS 'text' AND length(json_extract(package_json,'$.id'))>0 AND json_type(package_json,'$.name') IS 'text' AND length(json_extract(package_json,'$.name'))>0 AND (json_type(package_json,'$.version') IS NULL OR json_type(package_json,'$.version')='null' OR (json_type(package_json,'$.version')='text' AND length(json_extract(package_json,'$.version'))>0))),
 package_id TEXT GENERATED ALWAYS AS (json_extract(package_json,'$.id')) STORED,
 name TEXT GENERATED ALWAYS AS (json_extract(package_json,'$.name')) STORED,
 package_version TEXT GENERATED ALWAYS AS (json_extract(package_json,'$.version')) STORED,
 content_level TEXT NOT NULL CHECK(content_level IN('knowledge','full')),
 placement_json TEXT NOT NULL CHECK(json_valid(placement_json) AND json_type(placement_json)='object'),
 placement_kind TEXT GENERATED ALWAYS AS (json_extract(placement_json,'$.kind')) STORED NOT NULL,
 conversation_ref TEXT GENERATED ALWAYS AS (json_extract(placement_json,'$.conversation_ref')) STORED,
 content_digest TEXT NOT NULL CHECK(length(content_digest)=64),
 archive_material_id TEXT NOT NULL,installed_at TEXT NOT NULL,uninstalled_at TEXT,
 commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 FOREIGN KEY(namespace_id,archive_material_id) REFERENCES materials(namespace_id,id),
 CHECK((placement_kind='global' AND conversation_ref IS NULL) OR (placement_kind='chat' AND json_type(placement_json,'$.conversation_ref')='text' AND conversation_ref IS NOT NULL AND length(conversation_ref)>0))
);
CREATE UNIQUE INDEX IF NOT EXISTS active_bundle_package ON bundle_installations(namespace_id,subject_id,package_id,coalesce(package_version,''),placement_kind,coalesce(conversation_ref,'')) WHERE uninstalled_at IS NULL;
CREATE TABLE IF NOT EXISTS bundle_resources (
 installation_id TEXT NOT NULL REFERENCES bundle_installations(id),
 kind TEXT NOT NULL CHECK(kind IN('material','observation','memory','cognition','source_region','representation','derived_region','model_call')),
 original_id TEXT NOT NULL,local_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0),
 provenance TEXT NOT NULL CHECK(json_valid(provenance)),
 PRIMARY KEY(installation_id,kind,original_id,revision)
);
CREATE INDEX IF NOT EXISTS bundle_resource_owners ON bundle_resources(kind,local_id,installation_id);
CREATE TABLE IF NOT EXISTS bundle_ancestry (
 installation_id TEXT NOT NULL REFERENCES bundle_installations(id),
 source_installation_id TEXT NOT NULL,body TEXT NOT NULL CHECK(json_valid(body)),
 PRIMARY KEY(installation_id,source_installation_id)
);
CREATE TABLE IF NOT EXISTS bundle_vector_dispositions (
 installation_id TEXT NOT NULL REFERENCES bundle_installations(id),space_id TEXT NOT NULL,source_hash TEXT NOT NULL,
 source_key TEXT NOT NULL,body TEXT NOT NULL CHECK(json_valid(body)),
 PRIMARY KEY(installation_id,space_id,source_hash,source_key)
);
