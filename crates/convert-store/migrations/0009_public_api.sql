CREATE TABLE workspace_api_keys (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 owner_id uuid,
 credential_version bigint NOT NULL DEFAULT 0,
 name text NOT NULL,
 prefix text NOT NULL,
 secret_hash text NOT NULL UNIQUE,
 scopes text[] NOT NULL,
 expires_at timestamptz,
 revoked_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(),
 last_used_at timestamptz,
 version integer NOT NULL DEFAULT 1,
 rate_started_at timestamptz NOT NULL DEFAULT now(),
 rate_count integer NOT NULL DEFAULT 0,
 CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write']::text[] AND 'read'=ANY(scopes))
);
CREATE INDEX workspace_api_keys_list ON workspace_api_keys(workspace_id,created_at DESC);
CREATE TABLE api_key_events (
 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 key_id uuid NOT NULL REFERENCES workspace_api_keys(id),
 kind text NOT NULL,
 actor text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX api_key_events_workspace ON api_key_events(workspace_id,id DESC);
