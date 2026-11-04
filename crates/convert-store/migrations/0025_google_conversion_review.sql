-- Latest review of the saved Google Ads account's conversion actions: findings that may
-- double count with VisiConvert. Additive; read by the setup checklist so it never calls
-- Google, and ignored once the saved advertising account changes.
CREATE TABLE google_conversion_reviews (
 workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
 customer_id text NOT NULL CHECK (customer_id ~ '^[0-9]{10}$'),
 report jsonb NOT NULL,
 reviewed_at timestamptz NOT NULL DEFAULT now()
);
