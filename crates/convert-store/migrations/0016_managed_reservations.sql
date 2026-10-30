-- Managed purchase holds survive process crashes and settle by a durable outbox.
ALTER TABLE number_purchases
    ADD COLUMN managed boolean NOT NULL DEFAULT false,
    ADD COLUMN reservation_id bigint,
    ADD COLUMN reservation_settled_at timestamptz;
ALTER TABLE number_purchases ADD CHECK (reservation_id IS NULL OR (managed AND reservation_id > 0));
CREATE INDEX number_purchase_settlement ON number_purchases(last_attempt_at)
    WHERE managed AND reservation_id IS NOT NULL AND reservation_settled_at IS NULL;

UPDATE number_purchases p SET managed=true FROM signalwire_connections c
 WHERE p.workspace_id=c.workspace_id AND p.project_id=c.project_id AND c.managed;
UPDATE number_purchases SET status='review' WHERE managed AND status IN ('pending','cleanup');
