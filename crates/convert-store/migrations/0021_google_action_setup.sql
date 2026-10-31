-- Saved provider identities survive disconnects, renames and interrupted setup.
CREATE TABLE google_created_actions (
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    customer_id text NOT NULL CHECK (customer_id ~ '^[0-9]{10}$'),
    kind text NOT NULL CHECK (kind IN ('qualified', 'booking')),
    action_id text NOT NULL CHECK (action_id ~ '^[0-9]{1,20}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, customer_id, kind),
    UNIQUE (workspace_id, customer_id, action_id)
);
