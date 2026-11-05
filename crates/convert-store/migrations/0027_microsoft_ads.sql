-- Microsoft Advertising as a second destination for pipeline conversions. Additive only:
-- new tables and new nullable columns. Google connections, deliveries, events and their
-- existing columns are unchanged, and nothing is sent until an owner chooses a Microsoft
-- ad account and an action is mapped to a Microsoft offline conversion goal.

-- OAuth connection, chosen ad account and the latest read-only destination check.
CREATE TABLE microsoft_connections (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
    encrypted_token text,
    client_fingerprint text,
    connected_at timestamptz,
    pending_state_hash text,
    -- CustomerId header (the manager account operated from) and CustomerAccountId.
    customer_id text CHECK (customer_id ~ '^[0-9]{1,20}$'),
    account_id text CHECK (account_id ~ '^[0-9]{1,20}$'),
    account_name text CHECK (length(account_name) <= 300),
    destination_version integer NOT NULL DEFAULT 0 CHECK (destination_version >= 0),
    validation jsonb,
    validated_version integer,
    checked_at timestamptz,
    CHECK ((encrypted_token IS NULL) = (client_fingerprint IS NULL)),
    CHECK ((customer_id IS NULL) = (account_id IS NULL))
);
CREATE TABLE microsoft_oauth_attempts (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    state_hash text NOT NULL UNIQUE,
    browser_hash text NOT NULL,
    encrypted_verifier text NOT NULL,
    client_fingerprint text NOT NULL,
    session_hash text,
    operator_hash text,
    actor text NOT NULL,
    expected_revision bigint NOT NULL,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '10 minutes',
    CHECK ((session_hash IS NULL) <> (operator_hash IS NULL))
);

-- Offline conversion goals VisiConvert created, so an interrupted creation is recovered
-- instead of repeated, and so the two-hour wait before uploads is known.
CREATE TABLE microsoft_created_goals (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    account_id text NOT NULL CHECK (account_id ~ '^[0-9]{1,20}$'),
    action_key text NOT NULL CHECK (action_key ~ '^[a-z][a-z0-9_]{0,39}$'),
    goal_id text NOT NULL CHECK (goal_id ~ '^[0-9]{1,20}$'),
    goal_name text NOT NULL CHECK (length(goal_name) BETWEEN 1 AND 100),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, account_id, action_key)
);

-- Uploads reference the goal by name. Events recorded before the action was mapped (or
-- re-mapped) are never sent to Microsoft, so a cutover cannot double count with an
-- older uploader.
ALTER TABLE conversion_actions
    ADD COLUMN microsoft_goal_name text CHECK (length(microsoft_goal_name) BETWEEN 1 AND 100),
    ADD COLUMN microsoft_mapped_at timestamptz;

-- Microsoft planning state per event, tracked separately from the Google plan columns.
CREATE TABLE microsoft_event_plans (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    event_id uuid NOT NULL,
    revision integer NOT NULL CHECK (revision > 0),
    state text NOT NULL CHECK (state IN ('blocked','planned','final')),
    reason text CHECK (length(reason) <= 300),
    plan_after timestamptz NOT NULL DEFAULT now(),
    -- Click or event identity (counting rule); one event holds it per action.
    entity_key text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, event_id),
    FOREIGN KEY (workspace_id, event_id) REFERENCES pipeline_events(workspace_id, id)
);
CREATE UNIQUE INDEX microsoft_event_plans_entity ON microsoft_event_plans(workspace_id, entity_key) WHERE entity_key IS NOT NULL;
CREATE INDEX microsoft_event_plans_due ON microsoft_event_plans(plan_after) WHERE state = 'blocked';

-- One row per provider request: an upload, or a restatement or retraction of an upload.
-- A request is armed (attempted_at) before it leaves; an ambiguous send is never replayed.
CREATE TABLE microsoft_deliveries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    event_id uuid NOT NULL,
    event_revision integer NOT NULL CHECK (event_revision > 0),
    kind text NOT NULL CHECK (kind IN ('upload','restate','retract')),
    connection_revision bigint NOT NULL,
    snapshot jsonb NOT NULL,
    request jsonb NOT NULL,
    state text NOT NULL CHECK (state IN ('queued','sending','retry','processed','failed','uncertain','needs_review','canceled')),
    attempts integer NOT NULL DEFAULT 0,
    due_at timestamptz,
    attempted_at timestamptz,
    tracking_id text CHECK (length(tracking_id) <= 200),
    diagnostic text CHECK (length(diagnostic) <= 500),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id, event_id) REFERENCES pipeline_events(workspace_id, id)
);
-- One live request per event revision and kind; a canceled, never-sent row may be replanned.
CREATE UNIQUE INDEX microsoft_deliveries_revision ON microsoft_deliveries(workspace_id, event_id, event_revision, kind) WHERE state <> 'canceled';
CREATE INDEX microsoft_deliveries_due ON microsoft_deliveries(due_at) WHERE state IN ('queued','retry','sending');
CREATE INDEX microsoft_deliveries_event ON microsoft_deliveries(workspace_id, event_id, created_at DESC);

-- Same webhook events as Google deliveries; the payload names provider microsoft.
CREATE FUNCTION webhook_microsoft_conversion_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='processed' AND OLD.state<>'processed' THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.delivered','conversion',NEW.id::text,NEW.event_revision);
 ELSIF NEW.state IN ('failed','needs_review') AND OLD.state NOT IN ('failed','needs_review') THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.failed','conversion',NEW.id::text,NEW.event_revision);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_microsoft_conversion_update AFTER UPDATE OF state ON microsoft_deliveries FOR EACH ROW EXECUTE FUNCTION webhook_microsoft_conversion_change();
