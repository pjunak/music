"""baseline schema

Revision ID: 0001_baseline
Revises:
Create Date: 2026-04-24
"""
from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0001_baseline"
down_revision: str | None = None
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "users",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("username", sa.String(64), nullable=False, unique=True),
        sa.Column("password_hash", sa.String(255), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
    )

    op.create_table(
        "auth_sessions",
        sa.Column("token", sa.String(96), primary_key=True),
        sa.Column(
            "user_id",
            sa.Integer(),
            sa.ForeignKey("users.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("last_seen", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_auth_sessions_user_id", "auth_sessions", ["user_id"])

    op.create_table(
        "playlists",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("name", sa.String(256), nullable=False),
        sa.Column("mode_id", sa.String(64), nullable=True),
        sa.Column("source", sa.String(16), nullable=False),
        sa.Column("rules_json", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_playlists_mode_id", "playlists", ["mode_id"])

    op.create_table(
        "playlist_items",
        sa.Column(
            "playlist_id",
            sa.Integer(),
            sa.ForeignKey("playlists.id", ondelete="CASCADE"),
            primary_key=True,
        ),
        sa.Column("position", sa.Integer(), primary_key=True),
        sa.Column("beets_id", sa.Integer(), nullable=False),
        sa.Column("added_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_playlist_items_beets_id", "playlist_items", ["beets_id"])

    op.create_table(
        "global_nicknames",
        sa.Column("beets_id", sa.Integer(), primary_key=True),
        sa.Column("display_name", sa.String(512), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )

    op.create_table(
        "mode_nicknames",
        sa.Column("beets_id", sa.Integer(), primary_key=True),
        sa.Column("mode_id", sa.String(64), primary_key=True),
        sa.Column("display_name", sa.String(512), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )

    op.create_table(
        "playlist_nicknames",
        sa.Column("beets_id", sa.Integer(), primary_key=True),
        sa.Column(
            "playlist_id",
            sa.Integer(),
            sa.ForeignKey("playlists.id", ondelete="CASCADE"),
            primary_key=True,
        ),
        sa.Column("display_name", sa.String(512), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )

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

    op.create_table(
        "playback_state",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("state_json", sa.JSON(), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )


def downgrade() -> None:
    op.drop_table("playback_state")
    op.drop_index("ix_interrupts_mode_id", table_name="interrupts")
    op.drop_table("interrupts")
    op.drop_table("playlist_nicknames")
    op.drop_table("mode_nicknames")
    op.drop_table("global_nicknames")
    op.drop_index("ix_playlist_items_beets_id", table_name="playlist_items")
    op.drop_table("playlist_items")
    op.drop_index("ix_playlists_mode_id", table_name="playlists")
    op.drop_table("playlists")
    op.drop_index("ix_auth_sessions_user_id", table_name="auth_sessions")
    op.drop_table("auth_sessions")
    op.drop_table("users")
