-- Rails owns billing policy; Rust persists ordered, expiring authorizations.
CREATE TABLE managed_service_policies (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    project_id uuid NOT NULL,
    revision bigint NOT NULL CHECK(revision>0),
    state text NOT NULL CHECK(state IN ('active','grace','suspended')),
    reason text NOT NULL CHECK(reason IN ('paid','payment_required','canceled','review')),
    valid_until timestamptz NOT NULL,
    release_at timestamptz,
    payload jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    CHECK(release_at IS NULL OR (state='suspended' AND reason IN ('payment_required','canceled')))
);
-- This function deliberately treats missing/expired policies as suspended for
-- managed connections. BYOC/Twilio and self-hosted operators are unaffected.
CREATE FUNCTION managed_forwarding_allowed(w uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT NOT EXISTS(SELECT 1 FROM signalwire_connections c WHERE c.workspace_id=w AND c.managed)
      OR EXISTS(SELECT 1 FROM managed_service_policies p JOIN signalwire_connections c
        ON c.workspace_id=p.workspace_id AND c.project_id=p.project_id
        WHERE p.workspace_id=w AND c.managed AND p.state IN ('active','grace') AND p.valid_until>now())
$$;
CREATE FUNCTION billing_release_due(w uuid,n uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT EXISTS(SELECT 1 FROM managed_service_policies p
      JOIN signalwire_connections c ON c.workspace_id=p.workspace_id AND c.project_id=p.project_id AND c.managed
      JOIN number_purchases b ON b.workspace_id=p.workspace_id AND b.project_id=p.project_id AND b.managed AND b.status='registered'
      WHERE p.workspace_id=w AND b.number_id=n AND p.state='suspended' AND p.release_at<=now() AND p.valid_until>now())
$$;

-- Snapshot the decision at the irreversible boundary; later payment recovery
-- must not erase why a carrier deletion was started.
ALTER TABLE tracking_numbers ADD COLUMN release_policy jsonb;
