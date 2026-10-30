-- Preserve original purchase history; billing starts only when adoption commits.
CREATE TABLE managed_carrier_adoptions (
 workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
 adoption_id uuid NOT NULL UNIQUE,
 project_id uuid NOT NULL UNIQUE,
 plan jsonb NOT NULL,
 policy jsonb NOT NULL,
 receipt jsonb NOT NULL,
 activated_at timestamptz NOT NULL
);
ALTER TABLE tracking_numbers ADD COLUMN managed_adopted_at timestamptz;
ALTER TABLE tracking_numbers ADD CONSTRAINT adoption_after_purchase CHECK
 (managed_adopted_at IS NULL OR (purchased_at IS NOT NULL AND managed_adopted_at >= purchased_at));
