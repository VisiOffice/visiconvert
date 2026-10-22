-- Keep stable interaction IDs and all original payloads. Each existing interaction
-- starts as its own inquiry; no contact matching or automatic merging occurs.
ALTER TABLE leads ADD COLUMN inquiry_id uuid;
UPDATE leads SET inquiry_id=id;
ALTER TABLE leads ALTER COLUMN inquiry_id SET NOT NULL;
CREATE TABLE inquiries (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    primary_lead_id uuid NOT NULL,
    qualification text NOT NULL CHECK (qualification IN ('unreviewed','qualified','unqualified','spam')),
    notes text NOT NULL DEFAULT '',
    version integer NOT NULL DEFAULT 1 CHECK(version>0),
    created_at timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz,
    UNIQUE(workspace_id,id),
    FOREIGN KEY(workspace_id,primary_lead_id) REFERENCES leads(workspace_id,id) DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX inquiries_workspace_date ON inquiries(workspace_id,created_at DESC) WHERE archived_at IS NULL;
INSERT INTO inquiries(id,workspace_id,primary_lead_id,qualification,notes,version,created_at)
SELECT id,workspace_id,id,qualification,notes,version,created_at FROM leads;
ALTER TABLE leads ADD CONSTRAINT leads_inquiry_fk FOREIGN KEY(workspace_id,inquiry_id)
    REFERENCES inquiries(workspace_id,id) DEFERRABLE INITIALLY DEFERRED;
CREATE INDEX leads_inquiry_members ON leads(workspace_id,inquiry_id,created_at,id);
CREATE TABLE inquiry_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    workspace_id uuid NOT NULL,
    inquiry_id uuid NOT NULL,
    kind text NOT NULL,
    actor text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(workspace_id,inquiry_id) REFERENCES inquiries(workspace_id,id)
);
INSERT INTO inquiry_events(workspace_id,inquiry_id,kind,actor,payload) SELECT workspace_id,id,'inquiry_initialized','migration',jsonb_build_object('interaction_id',id) FROM leads;
CREATE INDEX inquiry_events_history ON inquiry_events(workspace_id,inquiry_id,id DESC);

-- AFTER INSERT avoids orphan inquiry rows when an idempotent lead insert uses
-- ON CONFLICT DO NOTHING. Every intake path, including carrier callbacks, uses it.
CREATE FUNCTION prepare_inquiry_id() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.inquiry_id := NEW.id;
    RETURN NEW;
END $$;
CREATE TRIGGER lead_inquiry_id BEFORE INSERT ON leads FOR EACH ROW EXECUTE FUNCTION prepare_inquiry_id();
CREATE FUNCTION create_initial_inquiry() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO inquiries(id,workspace_id,primary_lead_id,qualification,notes,version,created_at)
    VALUES(NEW.id,NEW.workspace_id,NEW.id,NEW.qualification,NEW.notes,NEW.version,NEW.created_at);
    INSERT INTO inquiry_events(workspace_id,inquiry_id,kind,actor,payload) VALUES(NEW.workspace_id,NEW.id,'inquiry_created','intake',jsonb_build_object('interaction_id',NEW.id,'channel',NEW.channel));
    RETURN NEW;
END $$;
CREATE TRIGGER lead_initial_inquiry AFTER INSERT ON leads FOR EACH ROW EXECUTE FUNCTION create_initial_inquiry();
