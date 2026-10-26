CREATE TABLE IF NOT EXISTS bundle_installations (
 id TEXT PRIMARY KEY NOT NULL, namespace_id TEXT NOT NULL,subject_id TEXT NOT NULL,
 source_namespace_id TEXT NOT NULL,source_subject_id TEXT NOT NULL,
 package_id TEXT NOT NULL,name TEXT NOT NULL,content_digest TEXT NOT NULL CHECK(length(content_digest)=64),
 archive_material_id TEXT NOT NULL,installed_at TEXT NOT NULL,uninstalled_at TEXT,
 commit_seq INTEGER NOT NULL CHECK(commit_seq>0),
 FOREIGN KEY(namespace_id,subject_id) REFERENCES subjects(namespace_id,id),
 FOREIGN KEY(namespace_id,archive_material_id) REFERENCES materials(namespace_id,id)
);
CREATE UNIQUE INDEX IF NOT EXISTS active_bundle_package ON bundle_installations(namespace_id,subject_id,package_id) WHERE uninstalled_at IS NULL;
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
