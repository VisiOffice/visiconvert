-- Existing tracking numbers and call identities remain Twilio-owned.
ALTER TABLE tracking_numbers ADD COLUMN provider text NOT NULL DEFAULT 'twilio'
    CHECK (provider IN ('twilio','signalwire'));
ALTER TABLE leads ADD COLUMN provider text CHECK (provider IN ('twilio','signalwire'));
UPDATE leads SET provider='twilio' WHERE channel='call';
ALTER TABLE workspaces ADD COLUMN signalwire_connection_revision bigint NOT NULL DEFAULT 0;
CREATE TABLE signalwire_connections (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    space_host text NOT NULL,
    project_id uuid NOT NULL,
    encrypted_token text NOT NULL,
    encrypted_signing_key text NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    verified_at timestamptz NOT NULL DEFAULT now(),
    last_webhook_at timestamptz
);
