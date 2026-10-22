CREATE TABLE sites (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    name text NOT NULL,
    origin text NOT NULL,
    allowed_fields text[] NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_session_at timestamptz,
    last_form_at timestamptz,
    session_count bigint NOT NULL DEFAULT 0,
    form_count bigint NOT NULL DEFAULT 0,
    rate_started_at timestamptz NOT NULL DEFAULT now(),
    rate_count integer NOT NULL DEFAULT 0,
    UNIQUE(workspace_id, id)
);
CREATE TABLE website_sessions (
    site_id uuid NOT NULL REFERENCES sites(id),
    id uuid NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(site_id, id)
);
CREATE INDEX website_sessions_expiry ON website_sessions(created_at);
CREATE TABLE website_submissions (
    site_id uuid NOT NULL,
    external_id text NOT NULL,
    session_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    lead_id uuid NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(site_id, external_id),
    FOREIGN KEY(site_id, session_id) REFERENCES website_sessions(site_id,id),
    FOREIGN KEY(workspace_id, site_id) REFERENCES sites(workspace_id,id),
    FOREIGN KEY(workspace_id, lead_id) REFERENCES leads(workspace_id,id)
);
CREATE INDEX website_submissions_lead ON website_submissions(workspace_id, lead_id);
