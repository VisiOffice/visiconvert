-- Signed outbound webhooks. Nothing is recorded until a workspace creates an endpoint.
ALTER TABLE workspace_api_keys DROP CONSTRAINT workspace_api_keys_scopes_check;
ALTER TABLE workspace_api_keys ADD CONSTRAINT workspace_api_keys_scopes_check
CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write','numbers:purchase','setup:write','webhooks:write']::text[] AND 'read'=ANY(scopes));

CREATE TABLE webhook_endpoints (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 url text NOT NULL CHECK (length(url) <= 2000),
 description text NOT NULL DEFAULT '' CHECK (length(description) <= 200),
 event_types text[] NOT NULL CHECK (cardinality(event_types) > 0),
 include_contact boolean NOT NULL DEFAULT true,
 encrypted_secret text NOT NULL,
 previous_encrypted_secret text,
 previous_secret_expires_at timestamptz,
 state text NOT NULL DEFAULT 'enabled' CHECK (state IN ('enabled','paused','disabled')),
 disabled_reason text,
 failing_since timestamptz,
 version integer NOT NULL DEFAULT 1 CHECK (version > 0),
 created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE (workspace_id, id)
);
CREATE INDEX webhook_endpoints_enabled ON webhook_endpoints(workspace_id) WHERE state = 'enabled';

CREATE TABLE webhook_endpoint_events (
 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 endpoint_id uuid NOT NULL,
 kind text NOT NULL,
 actor text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE webhook_events (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 type text NOT NULL,
 resource_kind text NOT NULL CHECK (resource_kind IN ('interaction','inquiry','booking','conversion','test')),
 resource_id text NOT NULL,
 resource_version integer,
 occurred_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE (workspace_id, id)
);
CREATE INDEX webhook_events_age ON webhook_events(occurred_at);

CREATE TABLE webhook_deliveries (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL,
 event_id uuid NOT NULL,
 endpoint_id uuid NOT NULL,
 payload jsonb,
 state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','sending','retry','succeeded','failed','canceled')),
 attempts integer NOT NULL DEFAULT 0,
 due_at timestamptz DEFAULT now(),
 last_status integer,
 last_error text CHECK (length(last_error) <= 300),
 last_attempt_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY (workspace_id, event_id) REFERENCES webhook_events(workspace_id, id) ON DELETE CASCADE,
 FOREIGN KEY (workspace_id, endpoint_id) REFERENCES webhook_endpoints(workspace_id, id) ON DELETE CASCADE,
 UNIQUE (event_id, endpoint_id)
);
CREATE INDEX webhook_delivery_due ON webhook_deliveries(due_at) WHERE state IN ('pending','retry','sending');
CREATE INDEX webhook_delivery_endpoint ON webhook_deliveries(endpoint_id, created_at DESC);

-- Record one event and fan it out to each enabled endpoint subscribed to its type.
CREATE FUNCTION webhook_emit(p_ws uuid, p_type text, p_kind text, p_id text, p_ver integer) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE v_event uuid;
BEGIN
 IF NOT EXISTS (SELECT 1 FROM webhook_endpoints e WHERE e.workspace_id=p_ws AND e.state='enabled' AND p_type=ANY(e.event_types)) THEN
  RETURN;
 END IF;
 v_event := gen_random_uuid();
 INSERT INTO webhook_events(id,workspace_id,type,resource_kind,resource_id,resource_version)
 VALUES (v_event,p_ws,p_type,p_kind,p_id,p_ver);
 INSERT INTO webhook_deliveries(id,workspace_id,event_id,endpoint_id)
 SELECT gen_random_uuid(),p_ws,v_event,e.id FROM webhook_endpoints e
 WHERE e.workspace_id=p_ws AND e.state='enabled' AND p_type=ANY(e.event_types);
 PERFORM pg_notify('visiconvert_webhooks','');
END $$;

CREATE FUNCTION webhook_lead_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.is_demo THEN RETURN NEW; END IF;
 IF TG_OP='INSERT' THEN
  PERFORM webhook_emit(NEW.workspace_id,'interaction.created','interaction',NEW.id::text,NULL);
 ELSIF NEW.channel='call' AND NEW.dial_terminal AND NOT OLD.dial_terminal THEN
  PERFORM webhook_emit(NEW.workspace_id,'call.completed','interaction',NEW.id::text,NULL);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_lead_insert AFTER INSERT ON leads FOR EACH ROW EXECUTE FUNCTION webhook_lead_change();
CREATE TRIGGER webhook_lead_update AFTER UPDATE OF dial_terminal ON leads FOR EACH ROW EXECUTE FUNCTION webhook_lead_change();

CREATE FUNCTION webhook_inquiry_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.qualification IS DISTINCT FROM NEW.qualification
  AND NOT EXISTS (SELECT 1 FROM leads WHERE workspace_id=NEW.workspace_id AND id=NEW.primary_lead_id AND is_demo) THEN
  PERFORM webhook_emit(NEW.workspace_id,'inquiry.reviewed','inquiry',NEW.id::text,NEW.version);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_inquiry_update AFTER UPDATE ON inquiries FOR EACH ROW EXECUTE FUNCTION webhook_inquiry_change();

CREATE FUNCTION webhook_outcome_change() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE v_type text;
BEGIN
 IF EXISTS (SELECT 1 FROM leads WHERE workspace_id=NEW.workspace_id AND id=NEW.lead_id AND is_demo) THEN
  RETURN NEW;
 END IF;
 IF TG_OP='INSERT' THEN
  v_type := CASE WHEN NEW.status='canceled' THEN 'booking.canceled' ELSE 'booking.recorded' END;
 ELSIF NEW.status='canceled' AND OLD.status<>'canceled' THEN
  v_type := 'booking.canceled';
 ELSIF NEW.status='booked' AND (NEW.revision<>OLD.revision OR OLD.status<>'booked') THEN
  v_type := 'booking.revised';
 ELSE
  RETURN NEW;
 END IF;
 PERFORM webhook_emit(NEW.workspace_id,v_type,'booking',NEW.external_id,NEW.revision);
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_outcome_write AFTER INSERT OR UPDATE ON outcomes FOR EACH ROW EXECUTE FUNCTION webhook_outcome_change();

CREATE FUNCTION webhook_conversion_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.state='processed' AND OLD.state<>'processed' THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.delivered','conversion',NEW.id::text,NEW.outbox_version);
 ELSIF NEW.state IN ('failed','needs_review') AND OLD.state NOT IN ('failed','needs_review') THEN
  PERFORM webhook_emit(NEW.workspace_id,'conversion.failed','conversion',NEW.id::text,NEW.outbox_version);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER webhook_conversion_update AFTER UPDATE OF state ON conversion_deliveries FOR EACH ROW EXECUTE FUNCTION webhook_conversion_change();
