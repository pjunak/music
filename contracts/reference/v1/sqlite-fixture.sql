PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE assistant_model_evaluations (
	role_id VARCHAR(64) NOT NULL,
	evaluation_id VARCHAR(128) NOT NULL,
	role_fingerprint VARCHAR(64) NOT NULL,
	status VARCHAR(16) NOT NULL,
	suite_id VARCHAR(64) NOT NULL,
	engine_id VARCHAR(128) NOT NULL,
	passed_cases INTEGER NOT NULL,
	total_cases INTEGER NOT NULL,
	job_id VARCHAR(32) NOT NULL,
	evaluated_at DATETIME NOT NULL,
	PRIMARY KEY (role_id, evaluation_id),
	FOREIGN KEY(role_id) REFERENCES assistant_model_roles (role_id) ON DELETE CASCADE,
	FOREIGN KEY(job_id) REFERENCES background_jobs (id) ON DELETE RESTRICT
);
INSERT INTO "assistant_model_evaluations" VALUES('playlist','fixture-evaluation','fixture-fingerprint','passed','playlist-model-v1','fixture-engine',1,1,'fixture-job','2026-08-27 12:34:56.000000');
CREATE TABLE assistant_model_roles (
	role_id VARCHAR(64) NOT NULL,
	connection_id VARCHAR(32) NOT NULL,
	model_id VARCHAR(256) NOT NULL,
	enabled BOOLEAN NOT NULL,
	timeout_seconds INTEGER NOT NULL,
	max_output_tokens INTEGER NOT NULL,
	thinking_mode VARCHAR(24) NOT NULL,
	conformance_status VARCHAR(16) NOT NULL,
	conformance_error_code VARCHAR(64),
	conformance_fingerprint VARCHAR(64),
	last_conformance_at DATETIME,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (role_id),
	FOREIGN KEY(connection_id) REFERENCES assistant_provider_connections (id) ON DELETE RESTRICT
);
INSERT INTO "assistant_model_roles" VALUES('playlist','0123456789abcdef0123456789abcdef','fixture-model',1,30,2048,'provider_default','passed',NULL,'fixture-fingerprint','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000');
CREATE TABLE assistant_provider_connections (
	id VARCHAR(32) NOT NULL,
	name VARCHAR(128) NOT NULL,
	adapter_id VARCHAR(64) NOT NULL,
	base_url VARCHAR(2048) NOT NULL,
	encrypted_api_key TEXT NOT NULL,
	api_key_nonce VARCHAR(64) NOT NULL,
	api_key_hint VARCHAR(32) NOT NULL,
	allow_private_network BOOLEAN NOT NULL,
	verification_status VARCHAR(16) NOT NULL,
	verification_error_code VARCHAR(64),
	verified_models_json TEXT NOT NULL,
	verified_capabilities_json TEXT NOT NULL,
	last_verified_at DATETIME,
	created_at DATETIME NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (id),
	UNIQUE (name)
);
INSERT INTO "assistant_provider_connections" VALUES('0123456789abcdef0123456789abcdef','Fixture provider','openai-responses','https://example.invalid/v1','IWuub7CXpzbsMf6m2owBQO258xmRViwZWxWA8Wc-ZuRVwaS_Lxkp-H_1I7o=','AAECAwQFBgcICQoL','••••cret',0,'verified',NULL,'["fixture-model"]','["structured-output"]','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000');
CREATE TABLE assistant_tag_vocabularies (
	"key" VARCHAR(32) NOT NULL,
	revision INTEGER NOT NULL,
	seed_version INTEGER NOT NULL,
	document_json TEXT NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY ("key")
);
INSERT INTO "assistant_tag_vocabularies" VALUES('default',1,1,'{"groups":[]}','2026-08-27 12:34:56.000000');
CREATE TABLE auth_sessions (
	token VARCHAR(96) NOT NULL,
	user_id INTEGER NOT NULL,
	created_at DATETIME NOT NULL,
	expires_at DATETIME NOT NULL,
	last_seen DATETIME NOT NULL,
	PRIMARY KEY (token),
	FOREIGN KEY(user_id) REFERENCES users (id) ON DELETE CASCADE
);
INSERT INTO "auth_sessions" VALUES('fixture-session-token',1,'2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000');
CREATE TABLE background_jobs (
	id VARCHAR(32) NOT NULL,
	kind VARCHAR(128) NOT NULL,
	status VARCHAR(32) NOT NULL,
	parameters_json TEXT NOT NULL,
	result_json TEXT,
	error TEXT,
	progress_current INTEGER NOT NULL,
	progress_total INTEGER,
	progress_phase VARCHAR(128) NOT NULL,
	progress_message VARCHAR(512) NOT NULL,
	attempts INTEGER NOT NULL,
	retry_of_id VARCHAR(32),
	created_at DATETIME NOT NULL,
	updated_at DATETIME NOT NULL,
	started_at DATETIME,
	finished_at DATETIME,
	PRIMARY KEY (id)
);
INSERT INTO "background_jobs" VALUES('fixture-job','library_context','succeeded','{"track_ids":[1]}','{"processed":1}',NULL,1,1,'complete','Fixture complete',1,NULL,'2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000');
CREATE TABLE cleanup_batches (
	id INTEGER NOT NULL,
	created_at DATETIME NOT NULL,
	scope_label VARCHAR(512) NOT NULL,
	items_json TEXT NOT NULL,
	reverted_at DATETIME,
	PRIMARY KEY (id)
);
INSERT INTO "cleanup_batches" VALUES(1,'2026-08-27 12:34:56.000000','Fixture scope','[{"track_id":1}]',NULL);
CREATE TABLE cleanup_name_lookups (
	id INTEGER NOT NULL,
	loose_key VARCHAR(512) NOT NULL,
	name VARCHAR(512) NOT NULL,
	artist_score INTEGER NOT NULL,
	album_score INTEGER NOT NULL,
	fetched_at DATETIME NOT NULL,
	PRIMARY KEY (id)
);
INSERT INTO "cleanup_name_lookups" VALUES(1,'fixture title','Fixture Title',90,80,'2026-08-27 12:34:56.000000');
CREATE TABLE playback_state (
	id INTEGER NOT NULL,
	state_json JSON NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (id)
);
INSERT INTO "playback_state" VALUES(1,'{"active_output_device_ids":["living-room"],"is_playing":false,"revision":7}','2026-08-27 12:34:56.000000');
CREATE TABLE playlist_items (
	playlist_id INTEGER NOT NULL,
	position INTEGER NOT NULL,
	track_id INTEGER NOT NULL,
	added_at DATETIME NOT NULL,
	PRIMARY KEY (playlist_id, position),
	FOREIGN KEY(playlist_id) REFERENCES playlists (id) ON DELETE CASCADE,
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "playlist_items" VALUES(1,0,1,'2026-08-27 12:34:56.000000');
CREATE TABLE playlists (
	id INTEGER NOT NULL,
	name VARCHAR(256) NOT NULL,
	mode_id VARCHAR(64),
	category VARCHAR(64),
	automatic_rule_json TEXT NOT NULL,
	automatic_source_signature VARCHAR(64),
	automatic_refreshed_at DATETIME,
	created_at DATETIME NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (id)
);
INSERT INTO "playlists" VALUES(1,'Fixture playlist','dnd','ambient','{}',NULL,NULL,'2026-08-27 12:34:56.000000','2026-08-27 12:34:56.000000');
CREATE TABLE track_analyses (
	track_id INTEGER NOT NULL,
	analyzer_id VARCHAR(128) NOT NULL,
	source_signature VARCHAR(64) NOT NULL,
	job_id VARCHAR(32) NOT NULL,
	energy FLOAT NOT NULL,
	brightness FLOAT NOT NULL,
	tension FLOAT NOT NULL,
	moods_json TEXT NOT NULL,
	evidence_json TEXT NOT NULL,
	metrics_json TEXT NOT NULL,
	confidence VARCHAR(16) NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (track_id, analyzer_id),
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "track_analyses" VALUES(1,'fixture-signal-v1','fixture-source','fixture-job',0.4,0.3,0.2,'["mood.calm"]','{"source":"fixture"}','{"rms":0.1}','medium','2026-08-27 12:34:56.000000');
CREATE TABLE track_analysis_failures (
	track_id INTEGER NOT NULL,
	analyzer_id VARCHAR(128) NOT NULL,
	source_signature VARCHAR(64) NOT NULL,
	job_id VARCHAR(32) NOT NULL,
	error TEXT NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (track_id, analyzer_id),
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "track_analysis_failures" VALUES(1,'fixture-failed-v1','fixture-source','fixture-job','synthetic failure','2026-08-27 12:34:56.000000');
CREATE TABLE track_analysis_tag_reviews (
	track_id INTEGER NOT NULL,
	analyzer_id VARCHAR(128) NOT NULL,
	tag VARCHAR(64) NOT NULL,
	source_signature VARCHAR(128) NOT NULL,
	decision VARCHAR(16) NOT NULL,
	reviewed_at DATETIME NOT NULL,
	PRIMARY KEY (track_id, analyzer_id, tag),
	CONSTRAINT ck_track_analysis_tag_reviews_decision CHECK (decision IN ('accepted', 'rejected')),
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "track_analysis_tag_reviews" VALUES(1,'fixture-signal-v1','mood.calm','fixture-source','accepted','2026-08-27 12:34:56.000000');
CREATE TABLE track_contexts (
	track_id INTEGER NOT NULL,
	analyzer_id VARCHAR(128) NOT NULL,
	source_signature VARCHAR(64) NOT NULL,
	job_id VARCHAR(32) NOT NULL,
	completeness VARCHAR(16) NOT NULL,
	confidence VARCHAR(16) NOT NULL,
	summary_json TEXT NOT NULL,
	timeline_json TEXT NOT NULL,
	sections_json TEXT NOT NULL,
	technical_json TEXT NOT NULL,
	stages_json TEXT NOT NULL,
	updated_at DATETIME NOT NULL,
	PRIMARY KEY (track_id, analyzer_id),
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "track_contexts" VALUES(1,'fixture-context-v1','fixture-source','fixture-job','complete','medium','{"label":"Fixture"}','[]','[]','{"duration_s":12.5}','{"signal":"complete"}','2026-08-27 12:34:56.000000');
CREATE TABLE track_user_tags (
	track_id INTEGER NOT NULL,
	tag VARCHAR(64) NOT NULL,
	created_at DATETIME NOT NULL,
	PRIMARY KEY (track_id, tag),
	FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
INSERT INTO "track_user_tags" VALUES(1,'mood.calm','2026-08-27 12:34:56.000000');
CREATE TABLE tracks (
	id INTEGER NOT NULL,
	path VARCHAR(1024) NOT NULL,
	title VARCHAR(512) NOT NULL,
	artist VARCHAR(512) NOT NULL,
	album_artist VARCHAR(512) NOT NULL,
	album VARCHAR(512) NOT NULL,
	track_no INTEGER,
	disc_no INTEGER,
	year INTEGER,
	genre VARCHAR(128) NOT NULL,
	length_s FLOAT NOT NULL,
	bpm INTEGER,
	display_title VARCHAR(512) NOT NULL,
	origin VARCHAR(512) NOT NULL,
	size_bytes INTEGER NOT NULL,
	mtime INTEGER NOT NULL,
	added_at DATETIME NOT NULL,
	PRIMARY KEY (id)
);
INSERT INTO "tracks" VALUES(1,'Fixture/track.wav','Fixture Track','Fixture Artist','Fixture Album Artist','Fixture Album',1,1,2026,'Soundtrack',12.5,120,'Fixture Artist — Fixture Track','fixture',24044,1787826896,'2026-08-27 12:34:56.000000');
CREATE TABLE users (
	id INTEGER NOT NULL,
	username VARCHAR(64) NOT NULL,
	password_hash VARCHAR(255) NOT NULL,
	created_at DATETIME NOT NULL,
	PRIMARY KEY (id),
	UNIQUE (username)
);
INSERT INTO "users" VALUES(1,'fixture-user','$argon2id$v=19$m=65536,t=3,p=4$cmV3cml0ZS1maXh0dXJlLXNhbHQ$tgdYDN8ijOk+7HZgF2oUm50+O8nOMKGggRmynxNu4ko','2026-08-27 12:34:56.000000');
CREATE INDEX ix_background_jobs_created_at ON background_jobs (created_at);
CREATE INDEX ix_background_jobs_kind ON background_jobs (kind);
CREATE INDEX ix_background_jobs_retry_of_id ON background_jobs (retry_of_id);
CREATE INDEX ix_background_jobs_status ON background_jobs (status);
CREATE UNIQUE INDEX ix_cleanup_name_lookups_loose_key ON cleanup_name_lookups (loose_key);
CREATE INDEX ix_playlists_category ON playlists (category);
CREATE INDEX ix_playlists_mode_id ON playlists (mode_id);
CREATE UNIQUE INDEX ix_tracks_path ON tracks (path);
CREATE INDEX ix_assistant_model_roles_connection_id ON assistant_model_roles (connection_id);
CREATE INDEX ix_auth_sessions_user_id ON auth_sessions (user_id);
CREATE INDEX ix_playlist_items_track_id ON playlist_items (track_id);
CREATE INDEX ix_track_analyses_job_id ON track_analyses (job_id);
CREATE INDEX ix_track_analysis_failures_job_id ON track_analysis_failures (job_id);
CREATE INDEX ix_track_contexts_job_id ON track_contexts (job_id);
CREATE INDEX ix_track_user_tags_tag ON track_user_tags (tag);
CREATE INDEX ix_assistant_model_evaluations_job_id ON assistant_model_evaluations (job_id);
COMMIT;
PRAGMA foreign_keys=ON;
