"""drop unused interrupts table

The Interrupt ORM model was a leftover from an earlier design where
interrupts were persisted DB rows. The current design holds interrupt
state ephemerally inside the sync state machine (see InterruptState in
app/sync/protocol.py) and reads predefined interrupt templates from
mode YAML manifests (InterruptSpec). Nothing has ever written to the
table — dropping it brings the schema in line with the code.

Revision ID: 0003_drop_interrupts
Revises: 0002_playlist_category
Create Date: 2026-04-27
"""
from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0003_drop_interrupts"
down_revision: str | None = "0002_playlist_category"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.drop_index("ix_interrupts_mode_id", table_name="interrupts")
    op.drop_table("interrupts")


def downgrade() -> None:
    op.create_table(
        "interrupts",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("mode_id", sa.String(64), nullable=False),
        sa.Column("name", sa.String(128), nullable=False),
        sa.Column(
            "playlist_id",
            sa.Integer(),
            sa.ForeignKey("playlists.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("soundboard_item", sa.String(512), nullable=True),
        sa.Column("fade_in_ms", sa.Integer(), nullable=False),
        sa.Column("fade_out_ms", sa.Integer(), nullable=False),
        sa.Column("return_to_ambient", sa.Boolean(), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.CheckConstraint(
            "playlist_id IS NOT NULL OR soundboard_item IS NOT NULL",
            name="ck_interrupts_has_target",
        ),
    )
    op.create_index("ix_interrupts_mode_id", "interrupts", ["mode_id"])
