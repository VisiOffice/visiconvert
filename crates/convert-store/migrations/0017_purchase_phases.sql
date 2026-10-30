-- Never infer that an old request did not reach the carrier. Old binaries omit
-- phase, so the default deliberately stays legacy during mixed-version upgrades.
ALTER TABLE number_purchases
    ADD COLUMN phase text NOT NULL DEFAULT 'legacy'
        CHECK (phase IN ('legacy','prepared','reserving','reserved','carrier')),
    ADD COLUMN carrier_started_at timestamptz,
    ADD COLUMN reservation_lookup_pending boolean NOT NULL DEFAULT false;
UPDATE number_purchases SET carrier_started_at=created_at
 WHERE phase='legacy';
CREATE INDEX number_purchase_attempts ON number_purchases(workspace_id,carrier_started_at);
CREATE INDEX number_purchase_reservation_lookup ON number_purchases(last_attempt_at)
 WHERE managed AND reservation_lookup_pending AND reservation_settled_at IS NULL;
ALTER TABLE number_purchases ADD CHECK (NOT reservation_lookup_pending OR managed);
