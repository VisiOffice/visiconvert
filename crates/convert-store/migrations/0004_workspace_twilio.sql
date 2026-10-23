-- Retain the counter when credentials are removed so stale requests cannot
-- match a new connection after a disconnect/reconnect cycle.
ALTER TABLE workspaces ADD COLUMN twilio_connection_revision bigint NOT NULL DEFAULT 0;

CREATE TABLE twilio_connections (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    account_sid text NOT NULL CHECK (account_sid ~ '^AC[0-9a-fA-F]{32}$'),
    encrypted_token text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    verified_at timestamptz NOT NULL DEFAULT now()
);
