CREATE TABLE hosted_users (
    user_id uuid PRIMARY KEY,
    credential_version bigint NOT NULL DEFAULT 0 CHECK (credential_version >= 0)
);
INSERT INTO hosted_users(user_id) SELECT DISTINCT user_id FROM hosted_memberships;
ALTER TABLE hosted_launch_codes ADD COLUMN credential_version bigint NOT NULL DEFAULT 0;
ALTER TABLE hosted_sessions ADD COLUMN credential_version bigint NOT NULL DEFAULT 0;
CREATE INDEX hosted_launch_user ON hosted_launch_codes(user_id);
CREATE INDEX hosted_session_user ON hosted_sessions(user_id);
