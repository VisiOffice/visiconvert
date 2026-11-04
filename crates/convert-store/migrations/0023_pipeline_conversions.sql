-- Automatic pipeline conversions. Additive: nothing already prepared is moved or requeued,
-- and nothing is sent until an owner turns on automatic sending.
ALTER TABLE workspace_api_keys DROP CONSTRAINT workspace_api_keys_scopes_check;
ALTER TABLE workspace_api_keys ADD CONSTRAINT workspace_api_keys_scopes_check
CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write','numbers:purchase','setup:write','webhooks:write','conversions:write']::text[] AND 'read'=ANY(scopes));

-- not_required_outside_eea is an owner attestation; who and when are kept with it.
ALTER TABLE conversion_delivery_settings
 ADD COLUMN auto_send boolean NOT NULL DEFAULT false,
 -- When auto_send was last turned on; earlier leads never become new_lead sends.
 ADD COLUMN auto_send_at timestamptz,
 ADD COLUMN consent_policy text NOT NULL DEFAULT 'per_lead' CHECK (consent_policy IN ('per_lead','not_required_outside_eea')),
 ADD COLUMN attested_by text,
 ADD COLUMN attested_at timestamptz,
 ADD CONSTRAINT conversion_consent_attested CHECK (consent_policy='per_lead' OR (attested_by IS NOT NULL AND attested_at IS NOT NULL));

CREATE TABLE conversion_actions (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 key text NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
 name text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
 trigger text NOT NULL CHECK (trigger IN ('events','new_lead')),
 default_value_minor bigint CHECK (default_value_minor BETWEEN 0 AND 100000000000),
 counting text NOT NULL CHECK (counting IN ('once_per_click','every')),
 google_action_id text CHECK (google_action_id ~ '^[0-9]{1,20}$'),
 enabled boolean NOT NULL DEFAULT true,
 version integer NOT NULL DEFAULT 1 CHECK (version > 0),
 created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE (workspace_id, key)
);

-- One row per business event; revisions replace the row and are kept in conversion_events.
-- Match inputs (phone, email) are never stored, only a digest for idempotent reposts.
CREATE TABLE pipeline_events (
 id uuid PRIMARY KEY,
 workspace_id uuid NOT NULL REFERENCES workspaces(id),
 action_key text NOT NULL,
 external_id text NOT NULL CHECK (length(external_id) BETWEEN 1 AND 150),
 lead_id uuid,
 matched_by text NOT NULL CHECK (matched_by IN ('lead_id','phone','email','new_lead','none')),
 attribution text NOT NULL CHECK (attribution IN ('first_click','latest_click')),
 match_digest text NOT NULL,
 value_minor bigint CHECK (value_minor BETWEEN 0 AND 100000000000),
 currency text NOT NULL DEFAULT 'USD' CHECK (currency = 'USD'),
 occurred_at timestamptz NOT NULL,
 revision integer NOT NULL DEFAULT 1 CHECK (revision > 0),
 status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','canceled')),
 -- Events recorded while automatic sending is off are kept for reporting, never sent.
 sendable boolean NOT NULL,
 plan_state text NOT NULL DEFAULT 'pending' CHECK (plan_state IN ('pending','blocked','planned','final')),
 plan_after timestamptz NOT NULL DEFAULT now(),
 blocked_reason text CHECK (length(blocked_reason) <= 300),
 outbox_id uuid,
 created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE (workspace_id, action_key, external_id),
 UNIQUE (workspace_id, id),
 FOREIGN KEY (workspace_id, action_key) REFERENCES conversion_actions(workspace_id, key),
 FOREIGN KEY (workspace_id, lead_id) REFERENCES leads(workspace_id, id),
 FOREIGN KEY (workspace_id, outbox_id) REFERENCES conversion_outbox(workspace_id, id)
);
CREATE INDEX pipeline_events_due ON pipeline_events(plan_after) WHERE plan_state IN ('pending','blocked');
CREATE INDEX pipeline_events_recent ON pipeline_events(workspace_id, created_at DESC, id DESC);

ALTER TABLE conversion_outbox DROP CONSTRAINT conversion_outbox_kind_check;
ALTER TABLE conversion_outbox ADD CONSTRAINT conversion_outbox_kind_check CHECK (kind IN ('qualified','booking','pipeline'));

-- Pipeline rows are revalidated per event by the planner and at send time. Excluding
-- them here keeps an unrelated booking or settings edit from stalling automatic sends.
CREATE OR REPLACE FUNCTION invalidate_conversion_preparation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    WITH changed AS (
        UPDATE conversion_outbox SET state='needs_review',version=version+1,updated_at=now()
        WHERE workspace_id=NEW.workspace_id AND state='awaiting_connection' AND kind<>'pipeline'
        RETURNING id
    )
    INSERT INTO conversion_events(workspace_id,kind,actor,payload)
    SELECT NEW.workspace_id,'invalidated','system',jsonb_build_object('id',id,'changed_table',TG_TABLE_NAME) FROM changed;
    RETURN NEW;
END $$;

-- Google actions VisiConvert created for a pipeline action, keyed 'pipeline:<key>', so a
-- retried or interrupted creation recovers the same action instead of creating another.
ALTER TABLE google_created_actions DROP CONSTRAINT google_created_actions_kind_check;
ALTER TABLE google_created_actions ADD CONSTRAINT google_created_actions_kind_check
CHECK (kind IN ('qualified','booking') OR kind ~ '^pipeline:[a-z][a-z0-9_]{0,39}$');
