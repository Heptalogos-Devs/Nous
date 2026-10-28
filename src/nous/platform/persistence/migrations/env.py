"""Alembic is run using the already configured authority connection."""

from alembic import context

connection = context.config.attributes["connection"]
context.configure(
    connection=connection,
    version_table="nous_alembic_version",
    target_metadata=context.config.attributes["domain_metadata"],
    transactional_ddl=True,
    render_as_batch=True,
)
with context.begin_transaction():
    context.run_migrations()
