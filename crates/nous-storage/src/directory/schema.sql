CREATE TABLE IF NOT EXISTS reference_addresses (
    address TEXT PRIMARY KEY NOT NULL,
    namespace_id TEXT NOT NULL REFERENCES namespaces(id),
    target TEXT NOT NULL CHECK(json_valid(target)),
    recorded_at TEXT NOT NULL,
    commit_seq INTEGER NOT NULL CHECK(commit_seq > 0),
    UNIQUE(namespace_id, target)
);

CREATE TABLE IF NOT EXISTS imported_reference_addresses (
    installation_id TEXT NOT NULL REFERENCES bundle_installations(id),
    address TEXT NOT NULL,
    target TEXT NOT NULL CHECK(json_valid(target)),
    PRIMARY KEY(installation_id,address,target)
);
CREATE INDEX IF NOT EXISTS imported_addresses_by_address ON imported_reference_addresses(address,installation_id);
