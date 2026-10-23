-- Pool membership is retained when disabled so old calls never fall back to a
-- misleading static campaign. Numbers cannot be moved between websites.
ALTER TABLE tracking_numbers ADD CONSTRAINT tracking_numbers_workspace_id_unique UNIQUE(workspace_id,id);
CREATE TABLE dni_pools (
    site_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    fallback_phone text NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    fallback_count bigint NOT NULL DEFAULT 0,
    UNIQUE(workspace_id,site_id),
    FOREIGN KEY(workspace_id,site_id) REFERENCES sites(workspace_id,id)
);
CREATE TABLE dni_pool_numbers (
    number_id uuid PRIMARY KEY,
    site_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    enabled boolean NOT NULL,
    UNIQUE(number_id,site_id,workspace_id),
    FOREIGN KEY(workspace_id,site_id) REFERENCES dni_pools(workspace_id,site_id),
    FOREIGN KEY(workspace_id,number_id) REFERENCES tracking_numbers(workspace_id,id)
);
CREATE TABLE dni_assignments (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    site_id uuid NOT NULL,
    session_id uuid NOT NULL,
    number_id uuid NOT NULL,
    assigned_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL,
    reusable_at timestamptz NOT NULL,
    revoked_at timestamptz,
    evidence jsonb NOT NULL,
    UNIQUE(site_id,session_id),
    FOREIGN KEY(number_id,site_id,workspace_id) REFERENCES dni_pool_numbers(number_id,site_id,workspace_id),
    FOREIGN KEY(site_id,session_id) REFERENCES website_sessions(site_id,id),
    CHECK(expires_at>assigned_at AND reusable_at>=expires_at)
);
CREATE INDEX dni_number_reservations ON dni_assignments(number_id,reusable_at DESC);
CREATE INDEX dni_site_history ON dni_assignments(workspace_id,site_id,assigned_at DESC);
CREATE TABLE dni_call_evidence (
    workspace_id uuid NOT NULL,
    lead_id uuid PRIMARY KEY,
    assignment_id uuid REFERENCES dni_assignments(id),
    status text NOT NULL,
    payload jsonb NOT NULL,
    FOREIGN KEY(workspace_id,lead_id) REFERENCES leads(workspace_id,id)
);
