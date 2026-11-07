-- Meta (Facebook and Instagram ads) as a third destination for pipeline conversions,
-- through the Conversions API. Additive only: new tables and new nullable columns.
-- Nothing is sent until the owner saves a dataset and access token, an action is mapped
-- to a Meta event, and delivery is on.

-- The owner-supplied dataset (Pixel) and Conversions API access token, the owner's
-- matching and Limited Data Use choices, and the latest read-only destination check.
CREATE TABLE meta_connections (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
    dataset_id text CHECK (dataset_id ~ '^[0-9]{1,20}$'),
    dataset_name text CHECK (length(dataset_name) <= 300),
    encrypted_token text,
    -- First 12 hex characters of the token's SHA-256, so the owner can tell tokens apart.
    token_fingerprint text CHECK (token_fingerprint ~ '^[0-9a-f]{12}$'),
    token_saved_at timestamptz,
    -- meta_clicks: only leads whose visit carried Meta click evidence. all_leads: every
    -- eligible lead, matched by hashed phone and email.
    match_mode text NOT NULL DEFAULT 'meta_clicks' CHECK (match_mode IN ('meta_clicks','all_leads')),
    -- geolocate: send Limited Data Use and let Meta decide by location.
    ldu text NOT NULL DEFAULT 'off' CHECK (ldu IN ('off','geolocate')),
    settings_version integer NOT NULL DEFAULT 0 CHECK (settings_version >= 0),
    validation jsonb,
    validated_revision bigint,
    checked_at timestamptz,
    CHECK ((encrypted_token IS NULL) = (dataset_id IS NULL)),
    CHECK ((encrypted_token IS NULL) = (token_fingerprint IS NULL))
);

-- The Meta event name each action is sent as. Events recorded before the action was
-- mapped (or re-mapped) are never sent to Meta.
ALTER TABLE conversion_actions
    ADD COLUMN meta_event_name text CHECK (meta_event_name ~ '^[A-Za-z][A-Za-z0-9_]{0,39}$'),
    ADD COLUMN meta_mapped_at timestamptz;

-- Meta planning state per event, separate from the Google and Microsoft plans.
CREATE TABLE meta_event_plans (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    event_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision > 0),
    state text NOT NULL CHECK (state IN ('blocked','planned','final')),
    reason text CHECK (length(reason) <= 300),
    plan_after timestamptz NOT NULL DEFAULT now(),
    -- Click, lead or event identity (counting rule); one event holds it per action.
    entity_key text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, event_id),
    FOREIGN KEY (workspace_id, event_id) REFERENCES pipeline_events(workspace_id, id)
);
CREATE UNIQUE INDEX meta_event_plans_entity ON meta_event_plans(workspace_id, entity_key) WHERE entity_key IS NOT NULL;
CREATE INDEX meta_event_plans_due ON meta_event_plans(plan_after) WHERE state = 'blocked';

-- One row per Conversions API request. Meta cannot restate or retract an event, so the
-- only kind is an upload. A request is armed (attempted_at) before it leaves; an
-- ambiguous send is never replayed.
CREATE TABLE meta_deliveries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    event_id uuid NOT NULL,
    event_revision integer NOT NULL CHECK (event_revision > 0),
    kind text NOT NULL CHECK (kind = 'upload'),
    connection_revision bigint NOT NULL,
    snapshot jsonb NOT NULL,
    request jsonb NOT NULL,
    state text NOT NULL CHECK (state IN ('queued','sending','retry','processed','failed','uncertain','needs_review','canceled')),
    attempts integer NOT NULL DEFAULT 0,
    due_at timestamptz,
    attempted_at timestamptz,
    -- Meta's fbtrace_id for the request.
    tracking_id text CHECK (length(tracking_id) <= 200),
    diagnostic text CHECK (length(diagnostic) <= 500),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id, event_id) REFERENCES pipeline_events(workspace_id, id)
);
CREATE UNIQUE INDEX meta_deliveries_revision ON meta_deliveries(workspace_id, event_id, event_revision, kind) WHERE state <> 'canceled';
CREATE INDEX meta_deliveries_due ON meta_deliveries(due_at) WHERE state IN ('queued','retry','sending');
CREATE INDEX meta_deliveries_event ON meta_deliveries(workspace_id, event_id, created_at DESC);

-- Browser details Meta uses to match website events. Kept only for workspaces with a Meta
-- connection, and cleared after 30 days.
ALTER TABLE website_sessions
    ADD COLUMN client_user_agent text CHECK (length(client_user_agent) <= 1000),
    ADD COLUMN client_ip inet;

CREATE FUNCTION webhook_meta_conversion_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='processed' AND OLD.state<>'processed' THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.delivered','conversion',NEW.id::text,NEW.event_revision);
 ELSIF NEW.state IN ('failed','needs_review') AND OLD.state NOT IN ('failed','needs_review') THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.failed','conversion',NEW.id::text,NEW.event_revision);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_meta_conversion_update AFTER UPDATE OF state ON meta_deliveries FOR EACH ROW EXECUTE FUNCTION webhook_meta_conversion_change();
