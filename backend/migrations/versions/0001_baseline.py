"""baseline schema

Single migration — the project was orphan-reset before this point, so
there's nothing earlier to preserve.

Revision ID: 0001_baseline
Revises:
Create Date: 2026-04-28
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
        "tracks",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("path", sa.String(1024), nullable=False, unique=True),
        sa.Column("title", sa.String(512), nullable=False, server_default=""),
        sa.Column("artist", sa.String(512), nullable=False, server_default=""),
        sa.Column("album_artist", sa.String(512), nullable=False, server_default=""),
        sa.Column("album", sa.String(512), nullable=False, server_default=""),
        sa.Column("track_no", sa.Integer(), nullable=True),
        sa.Column("disc_no", sa.Integer(), nullable=True),
        sa.Column("year", sa.Integer(), nullable=True),
        sa.Column("genre", sa.String(128), nullable=False, server_default=""),
        sa.Column("length_s", sa.Float(), nullable=False, server_default="0"),
        sa.Column("bpm", sa.Integer(), nullable=True),
        sa.Column("size_bytes", sa.Integer(), nullable=False),
        sa.Column("mtime", sa.Integer(), nullable=False),
        sa.Column("added_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_tracks_path", "tracks", ["path"])

    op.create_table(
        "playlists",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("name", sa.String(256), nullable=False),
        sa.Column("mode_id", sa.String(64), nullable=True),
        sa.Column("category", sa.String(64), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_playlists_mode_id", "playlists", ["mode_id"])
    op.create_index("ix_playlists_category", "playlists", ["category"])

    op.create_table(
        "playlist_items",
        sa.Column(
            "playlist_id",
            sa.Integer(),
            sa.ForeignKey("playlists.id", ondelete="CASCADE"),
            primary_key=True,
        ),
        sa.Column("position", sa.Integer(), primary_key=True),
        sa.Column(
            "track_id",
            sa.Integer(),
            sa.ForeignKey("tracks.id", ondelete="CASCADE"),
            nullable=False,
        ),
        sa.Column("added_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("ix_playlist_items_track_id", "playlist_items", ["track_id"])

    op.create_table(
        "playback_state",
        sa.Column("id", sa.Integer(), primary_key=True),
        sa.Column("state_json", sa.JSON(), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
    )


def downgrade() -> None:
    op.drop_table("playback_state")
    op.drop_index("ix_playlist_items_track_id", table_name="playlist_items")
    op.drop_table("playlist_items")
    op.drop_index("ix_playlists_category", table_name="playlists")
    op.drop_index("ix_playlists_mode_id", table_name="playlists")
    op.drop_table("playlists")
    op.drop_index("ix_tracks_path", table_name="tracks")
    op.drop_table("tracks")
    op.drop_index("ix_auth_sessions_user_id", table_name="auth_sessions")
    op.drop_table("auth_sessions")
    op.drop_table("users")
