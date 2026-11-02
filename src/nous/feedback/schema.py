from sqlalchemy import JSON, Column, String, Table

from nous.platform.persistence import metadata

tasks = Table(
    "cognitive_feedback_tasks",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    
    Column("record", JSON, nullable=False),
)
