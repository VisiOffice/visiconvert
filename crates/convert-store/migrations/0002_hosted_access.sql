-- Optional cloud bridge. Self-hosted operator-key access has no cloud dependency.
CREATE TABLE hosted_workspaces (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    owner_id uuid NOT NULL,
    enabled boolean NOT NULL DEFAULT true
);
CREATE TABLE hosted_memberships (
    workspace_id uuid NOT NULL REFERENCES hosted_workspaces(workspace_id),
    user_id uuid NOT NULL,
    role text NOT NULL CHECK (role IN ('owner')),
    enabled boolean NOT NULL DEFAULT true,
    PRIMARY KEY (workspace_id, user_id)
);
CREATE TABLE hosted_launch_codes (
    code_hash text PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    browser_challenge text NOT NULL,
    expires_at timestamptz NOT NULL,
    FOREIGN KEY (workspace_id,user_id) REFERENCES hosted_memberships(workspace_id,user_id) ON DELETE CASCADE
);
CREATE INDEX hosted_launch_expiry ON hosted_launch_codes(expires_at);
CREATE TABLE hosted_sessions (
    token_hash text PRIMARY KEY,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    expires_at timestamptz NOT NULL,
    FOREIGN KEY (workspace_id,user_id) REFERENCES hosted_memberships(workspace_id,user_id) ON DELETE CASCADE
);
CREATE INDEX hosted_session_expiry ON hosted_sessions(expires_at);
