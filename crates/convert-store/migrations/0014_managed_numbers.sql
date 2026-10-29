-- Numbers bought through VisiConvert keep their row after release so call history,
-- pool membership and DNI evidence stay intact. Only live numbers must be unique.
ALTER TABLE signalwire_connections ADD COLUMN managed boolean NOT NULL DEFAULT false;
ALTER TABLE tracking_numbers
    ADD COLUMN carrier_sid text,
    ADD COLUMN purchased_at timestamptz,
    ADD COLUMN release_requested_at timestamptz,
    ADD COLUMN released_at timestamptz,
    ADD CHECK (released_at IS NULL OR release_requested_at IS NOT NULL),
    ADD CHECK ((purchased_at IS NULL) = (carrier_sid IS NULL));
ALTER TABLE tracking_numbers DROP CONSTRAINT tracking_numbers_phone_number_key;
CREATE UNIQUE INDEX tracking_numbers_live_phone ON tracking_numbers(phone_number) WHERE released_at IS NULL;
CREATE INDEX tracking_numbers_release_due ON tracking_numbers(release_requested_at)
    WHERE release_requested_at IS NOT NULL AND released_at IS NULL;
-- A purchase is one idempotent request: retries with the same ID never buy twice.
CREATE TABLE number_purchases (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    request_id uuid NOT NULL,
    phone_number text NOT NULL,
    status text NOT NULL CHECK (status IN ('pending','registered','failed')),
    number_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, request_id),
    FOREIGN KEY (workspace_id, number_id) REFERENCES tracking_numbers(workspace_id, id)
);
-- Buying numbers is billable, so API keys need a separate permission for it.
ALTER TABLE workspace_api_keys DROP CONSTRAINT workspace_api_keys_scopes_check;
ALTER TABLE workspace_api_keys ADD CONSTRAINT workspace_api_keys_scopes_check
CHECK(scopes <@ ARRAY['read','leads:write','bookings:write','numbers:write','numbers:purchase','setup:write']::text[] AND 'read'=ANY(scopes));
