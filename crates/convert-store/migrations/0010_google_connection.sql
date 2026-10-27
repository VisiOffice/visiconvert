-- Connection/discovery only. No provider delivery state or worker is enabled.
CREATE TABLE google_connections (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
    encrypted_token text,
    client_fingerprint text,
    connected_at timestamptz,
    validation jsonb,
    validated_settings_version integer,
    checked_at timestamptz,
    pending_state_hash text,
    CHECK ((encrypted_token IS NULL) = (client_fingerprint IS NULL))
);
CREATE TABLE google_oauth_attempts (
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
CREATE FUNCTION invalidate_google_validation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE google_connections SET validation=NULL,validated_settings_version=NULL,checked_at=NULL
    WHERE workspace_id=NEW.workspace_id;
    RETURN NEW;
END $$;
CREATE TRIGGER google_destination_change AFTER INSERT OR UPDATE ON conversion_settings
FOR EACH ROW EXECUTE FUNCTION invalidate_google_validation();
