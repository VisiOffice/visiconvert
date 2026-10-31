-- Delivery is opt-in. Existing preparations are never automatically queued.
CREATE TABLE conversion_delivery_settings (
 workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
 enabled boolean NOT NULL DEFAULT false,
 version integer NOT NULL DEFAULT 1 CHECK(version>0)
);
ALTER TABLE conversion_outbox ADD CONSTRAINT conversion_outbox_workspace_id UNIQUE(workspace_id,id);
CREATE TABLE conversion_deliveries (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 outbox_id uuid NOT NULL,
 outbox_version integer NOT NULL,
 provider text NOT NULL DEFAULT 'google' CHECK(provider='google'),
 connection_revision bigint NOT NULL,
 snapshot jsonb NOT NULL,
 request jsonb NOT NULL,
 state text NOT NULL CHECK(state IN ('validated','queued','sending','retry','submitted','polling','processed','failed','uncertain','needs_review','canceled')),
 review_required boolean NOT NULL DEFAULT false,
 request_id text,
 attempts integer NOT NULL DEFAULT 0,
 polls integer NOT NULL DEFAULT 0,
 due_at timestamptz,
 attempted_at timestamptz,
 validated_at timestamptz NOT NULL DEFAULT now(),
 diagnostic text,
 warnings boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(workspace_id,outbox_id) REFERENCES conversion_outbox(workspace_id,id),
 UNIQUE(workspace_id,outbox_id,outbox_version)
);
CREATE INDEX conversion_delivery_due ON conversion_deliveries(due_at) WHERE state IN ('queued','retry','submitted','sending','polling');
-- Source edits invalidate unsent work, while receipts remain available to poll.
CREATE FUNCTION invalidate_conversion_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 UPDATE conversion_deliveries SET review_required=true,
 state=CASE WHEN attempted_at IS NULL THEN 'needs_review' ELSE state END,
 diagnostic='Source changed; review required. Already sent conversions are not retracted.',updated_at=now()
 WHERE workspace_id=NEW.workspace_id AND outbox_id=NEW.id AND outbox_version<>NEW.version;
 RETURN NEW;
END $$;
CREATE TRIGGER conversion_delivery_source_change AFTER UPDATE ON conversion_outbox
FOR EACH ROW WHEN (OLD.version IS DISTINCT FROM NEW.version) EXECUTE FUNCTION invalidate_conversion_delivery();
