-- Once carrier deletion may have started, restoration is no longer safe. Keep the
-- intent across timeouts/crashes so the next worker retries the same carrier SID.
ALTER TABLE tracking_numbers ADD COLUMN release_started_at timestamptz;
ALTER TABLE tracking_numbers ADD CHECK (release_started_at IS NULL OR release_requested_at IS NOT NULL);

-- Persist everything needed to identify/recover a purchase BEFORE the carrier call.
ALTER TABLE number_purchases
    ADD COLUMN request_payload jsonb,
    ADD COLUMN space_host text,
    ADD COLUMN project_id uuid,
    ADD COLUMN carrier_sid text,
    ADD COLUMN last_attempt_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE number_purchases DROP CONSTRAINT number_purchases_status_check;
ALTER TABLE number_purchases ADD CHECK(status IN ('pending','registered','failed','cleanup','review'));
-- Old failed attempts may have bought a number whose response was lost. They have
-- no durable carrier evidence; reserve their capacity until an operator reconciles them.
UPDATE number_purchases SET status='review' WHERE status IN ('pending','failed');
CREATE INDEX number_purchases_recovery ON number_purchases(last_attempt_at)
    WHERE status IN ('pending','cleanup');
