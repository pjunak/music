"""add playlists.category

Revision ID: 0002_playlist_category
Revises: 0001_baseline
Create Date: 2026-04-24
"""
from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0002_playlist_category"
down_revision: str | None = "0001_baseline"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    with op.batch_alter_table("playlists") as batch:
        batch.add_column(sa.Column("category", sa.String(64), nullable=True))
    op.create_index("ix_playlists_category", "playlists", ["category"])


def downgrade() -> None:
    op.drop_index("ix_playlists_category", table_name="playlists")
    with op.batch_alter_table("playlists") as batch:
        batch.drop_column("category")
