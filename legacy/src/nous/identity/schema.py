"""Namespace ownership and stable host bindings in the Authority database."""
from sqlalchemy import JSON, Boolean, Column, ForeignKey, Integer, String, Table, text
from nous.platform.persistence.schema import metadata

namespaces = Table(
    'namespaces', metadata,
    Column('id', String, primary_key=True),
    Column('slug', String, nullable=False, unique=True),
    Column('display_name', String, nullable=False),
    Column('mode', String, nullable=False),
    Column('subject_key', String, unique=True),
    Column('enabled_features', JSON, nullable=False),
    Column('state', String, nullable=False, server_default='active'),
    Column('revision', Integer, nullable=False, server_default=text('1')),
    Column('commit_seq', Integer, nullable=False, server_default=text('0')),
    Column('created_at', String), Column('updated_at', String),
    Column('purging', Boolean, nullable=False, server_default=text('0')),
)
subject_bindings = Table(
    'subject_bindings', metadata,
    Column('host_id', String, primary_key=True),
    Column('external_id', String, primary_key=True),
    Column('namespace_id', String, ForeignKey('namespaces.id'), nullable=False, index=True),
    Column('commit_seq', Integer, nullable=False),
)
retired = Table(
    'identity_retirements', metadata,
    Column('kind', String, primary_key=True), Column('id', String, primary_key=True),
    Column('namespace_id', String, nullable=False, index=True),
    Column('purged_at', String, nullable=False), Column('commit_seq', Integer, nullable=False),
)
