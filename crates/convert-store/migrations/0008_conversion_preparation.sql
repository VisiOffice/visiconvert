-- Preparation only: no provider credentials, worker, or sending state yet.
CREATE TABLE conversion_settings (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    configuration jsonb NOT NULL,
    version integer NOT NULL CHECK(version>0)
);
CREATE TABLE conversion_consents (
    workspace_id uuid NOT NULL,
    lead_id uuid NOT NULL,
    evidence jsonb NOT NULL,
    version integer NOT NULL CHECK(version>0),
    PRIMARY KEY(workspace_id,lead_id),
    FOREIGN KEY(workspace_id,lead_id) REFERENCES leads(workspace_id,id)
);
CREATE TABLE conversion_outbox (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    kind text NOT NULL CHECK(kind IN ('qualified','booking')),
    entity_key text NOT NULL,
    transaction_id text NOT NULL,
    fingerprint text NOT NULL,
    snapshot jsonb NOT NULL,
    state text NOT NULL CHECK(state IN ('awaiting_connection','needs_review','canceled')),
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(workspace_id,kind,entity_key),
    UNIQUE(workspace_id,transaction_id)
);
CREATE INDEX conversion_outbox_workspace ON conversion_outbox(workspace_id,updated_at DESC);
CREATE TABLE conversion_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    kind text NOT NULL,
    actor text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX conversion_events_workspace ON conversion_events(workspace_id,id DESC);

-- Conservatively invalidate all prepared conversions in this workspace. Keeping
-- the frozen snapshot allows review; nothing is silently retargeted or resent.
CREATE FUNCTION invalidate_conversion_preparation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    WITH changed AS (
        UPDATE conversion_outbox SET state='needs_review',version=version+1,updated_at=now()
        WHERE workspace_id=NEW.workspace_id AND state='awaiting_connection'
        RETURNING id
    )
    INSERT INTO conversion_events(workspace_id,kind,actor,payload)
    SELECT NEW.workspace_id,'invalidated','system',jsonb_build_object('id',id,'changed_table',TG_TABLE_NAME) FROM changed;
    RETURN NEW;
END $$;
CREATE TRIGGER conversion_inquiry_change AFTER UPDATE ON inquiries FOR EACH ROW
WHEN (OLD.qualification IS DISTINCT FROM NEW.qualification OR OLD.primary_lead_id IS DISTINCT FROM NEW.primary_lead_id OR OLD.archived_at IS DISTINCT FROM NEW.archived_at)
EXECUTE FUNCTION invalidate_conversion_preparation();
CREATE TRIGGER conversion_group_change AFTER UPDATE ON leads FOR EACH ROW
WHEN (OLD.inquiry_id IS DISTINCT FROM NEW.inquiry_id)
EXECUTE FUNCTION invalidate_conversion_preparation();
CREATE TRIGGER conversion_booking_change AFTER INSERT OR UPDATE ON outcomes FOR EACH ROW EXECUTE FUNCTION invalidate_conversion_preparation();
CREATE TRIGGER conversion_settings_change AFTER INSERT OR UPDATE ON conversion_settings FOR EACH ROW EXECUTE FUNCTION invalidate_conversion_preparation();
CREATE TRIGGER conversion_consent_change AFTER INSERT OR UPDATE ON conversion_consents FOR EACH ROW EXECUTE FUNCTION invalidate_conversion_preparation();

CREATE INDEX conversion_booking_event_time ON audit_events(workspace_id,(payload->>'external_id'),created_at)
WHERE kind='outcome_updated' AND payload->>'revision'='1';
