"""Initial immutable authority schema. Subsequent changes use new revisions.

Revision ID: 0001_authority
"""

from alembic import op

revision = "0001_authority"
down_revision = None
branch_labels = None
depends_on = None

_TABLES = (
    (
        "bundle_installs",
        "CREATE TABLE bundle_installs (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "cognitive_contexts",
        "CREATE TABLE cognitive_contexts (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "cognitive_revisions",
        "CREATE TABLE cognitive_revisions (\n\tid VARCHAR NOT NULL, \n\trevision INTEGER NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\tkind VARCHAR NOT NULL, \n\thead BOOLEAN NOT NULL, \n\trecord JSON NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (id, revision)\n)",
    ),
    (
        "cognitive_vectors",
        "CREATE TABLE cognitive_vectors (\n\treference VARCHAR NOT NULL, \n\tspace VARCHAR NOT NULL, \n\tmodel VARCHAR NOT NULL, \n\tdimensions INTEGER NOT NULL, \n\tvector JSON NOT NULL, \n\tcall_id VARCHAR, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (reference, space)\n)",
    ),
    (
        "context_feedback",
        "CREATE TABLE context_feedback (\n\tid VARCHAR NOT NULL, \n\tcontext_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_catalogs",
        "CREATE TABLE import_catalogs (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_file_successes",
        "CREATE TABLE import_file_successes (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\tinput_source_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_inputs",
        "CREATE TABLE import_inputs (\n\tid VARCHAR NOT NULL, \n\tcatalog_id VARCHAR NOT NULL, \n\tmaterial_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_manifest_heads",
        'CREATE TABLE import_manifest_heads (\n\t"key" VARCHAR NOT NULL, \n\tsuccess_id VARCHAR NOT NULL, \n\tPRIMARY KEY ("key")\n)',
    ),
    (
        "import_outputs",
        "CREATE TABLE import_outputs (\n\tid VARCHAR NOT NULL, \n\tunit_id VARCHAR NOT NULL, \n\toperation_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_requests",
        "CREATE TABLE import_requests (\n\tid VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "import_units",
        "CREATE TABLE import_units (\n\tid VARCHAR NOT NULL, \n\tcatalog_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "materials",
        "CREATE TABLE materials (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tcontent_hash VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "namespaces",
        "CREATE TABLE namespaces (\n\tid VARCHAR NOT NULL, \n\tname VARCHAR NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "nous_commit_clock",
        "CREATE TABLE nous_commit_clock (\n\tsingleton INTEGER NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (singleton)\n)",
    ),
    (
        "nous_configuration",
        'CREATE TABLE nous_configuration (\n\tscope VARCHAR NOT NULL, \n\t"key" VARCHAR NOT NULL, \n\tvalue JSON NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (scope, "key")\n)',
    ),
    (
        "nous_configuration_history",
        'CREATE TABLE nous_configuration_history (\n\tcommit_seq INTEGER NOT NULL, \n\tscope VARCHAR NOT NULL, \n\t"key" VARCHAR NOT NULL, \n\tvalue JSON, \n\tPRIMARY KEY (commit_seq)\n)',
    ),
    (
        "nous_job_links",
        "CREATE TABLE nous_job_links (\n\tworkflow_id VARCHAR NOT NULL, \n\toperation_key VARCHAR NOT NULL, \n\tcaller_id VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR, \n\tscope VARCHAR NOT NULL, \n\towned_parent VARCHAR, \n\tworkflow_name VARCHAR NOT NULL, \n\tqueue_name VARCHAR NOT NULL, \n\tPRIMARY KEY (workflow_id)\n)",
    ),
    (
        "nous_model_calls",
        "CREATE TABLE nous_model_calls (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR, \n\tpurpose VARCHAR NOT NULL, \n\trequest_hash VARCHAR NOT NULL, \n\tday VARCHAR NOT NULL, \n\tstatus VARCHAR NOT NULL, \n\trequest JSON NOT NULL, \n\tresult JSON, \n\tusage JSON, \n\terror_code VARCHAR, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "nous_operations",
        "CREATE TABLE nous_operations (\n\toperation_key VARCHAR NOT NULL, \n\tcaller_id VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tscope VARCHAR NOT NULL, \n\toperation_id VARCHAR NOT NULL, \n\toperation_kind VARCHAR NOT NULL, \n\tpayload_hash VARCHAR NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\trecorded_at VARCHAR NOT NULL, \n\tresult_json TEXT NOT NULL, \n\tjobs_json TEXT NOT NULL, \n\tPRIMARY KEY (operation_key), \n\tUNIQUE (caller_id, namespace_id, scope, operation_id), \n\tUNIQUE (commit_seq)\n)",
    ),
    (
        "nous_prompts",
        "CREATE TABLE nous_prompts (\n\tid VARCHAR NOT NULL, \n\trevision INTEGER NOT NULL, \n\ttemplate TEXT NOT NULL, \n\tactive BOOLEAN NOT NULL, \n\tPRIMARY KEY (id, revision)\n)",
    ),
    (
        "observations",
        "CREATE TABLE observations (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\tmaterial_id VARCHAR NOT NULL, \n\tkind VARCHAR NOT NULL, \n\tactive BOOLEAN NOT NULL, \n\trecord JSON NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "source_operations",
        "CREATE TABLE source_operations (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tsubject_id VARCHAR NOT NULL, \n\trecord JSON NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
    (
        "subjects",
        "CREATE TABLE subjects (\n\tid VARCHAR NOT NULL, \n\tnamespace_id VARCHAR NOT NULL, \n\tname VARCHAR NOT NULL, \n\tcapabilities JSON NOT NULL, \n\tcommit_seq INTEGER NOT NULL, \n\tPRIMARY KEY (id)\n)",
    ),
)
_INDEXES = (
    "CREATE INDEX ix_cognitive_revisions_kind ON cognitive_revisions (kind)",
    "CREATE INDEX ix_cognitive_revisions_namespace_id ON cognitive_revisions (namespace_id)",
    "CREATE INDEX ix_cognitive_revisions_subject_id ON cognitive_revisions (subject_id)",
    "CREATE INDEX ix_import_inputs_catalog_id ON import_inputs (catalog_id)",
    "CREATE INDEX ix_import_outputs_unit_id ON import_outputs (unit_id)",
    "CREATE INDEX ix_import_units_catalog_id ON import_units (catalog_id)",
    "CREATE INDEX ix_materials_content_hash ON materials (content_hash)",
    "CREATE INDEX ix_materials_namespace_id ON materials (namespace_id)",
    "CREATE INDEX ix_nous_job_links_operation_key ON nous_job_links (operation_key)",
    "CREATE INDEX ix_nous_job_links_owned_parent ON nous_job_links (owned_parent)",
    "CREATE INDEX ix_nous_model_calls_request_hash ON nous_model_calls (request_hash)",
    "CREATE INDEX ix_observations_material_id ON observations (material_id)",
    "CREATE INDEX ix_observations_subject_id ON observations (subject_id)",
    "CREATE INDEX ix_subjects_namespace_id ON subjects (namespace_id)",
)


def upgrade() -> None:
    for _name, statement in _TABLES:
        op.execute(statement)
    for statement in _INDEXES:
        op.execute(statement)
    op.execute("INSERT INTO nous_commit_clock(singleton,commit_seq) VALUES (1,0)")


def downgrade() -> None:
    for name, _statement in reversed(_TABLES):
        op.drop_table(name)
