CREATE TABLE workspaces (id uuid PRIMARY KEY, name text NOT NULL);
CREATE TABLE tracking_numbers (
    id uuid PRIMARY KEY, workspace_id uuid NOT NULL REFERENCES workspaces(id),
    label text NOT NULL, phone_number text NOT NULL UNIQUE, destination text NOT NULL,
    source text NOT NULL, campaign text NOT NULL, is_demo boolean NOT NULL DEFAULT false,
    CHECK (phone_number <> destination)
);
CREATE TABLE leads (
    id uuid PRIMARY KEY, workspace_id uuid NOT NULL REFERENCES workspaces(id),
    external_id text NOT NULL, name text NOT NULL, contact text NOT NULL, submission_payload jsonb,
    channel text NOT NULL CHECK (channel IN ('call', 'form')),
    source text NOT NULL, campaign text NOT NULL,
    qualification text NOT NULL DEFAULT 'unreviewed' CHECK (qualification IN ('unreviewed','qualified','unqualified','spam')),
    notes text NOT NULL DEFAULT '', version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(), is_demo boolean NOT NULL DEFAULT false,
    tracking_number text, destination text, call_status text, parent_status text,
    parent_sequence integer NOT NULL DEFAULT -1, dial_terminal boolean NOT NULL DEFAULT false,
    duration_seconds integer CHECK (duration_seconds >= 0),
    UNIQUE (workspace_id, channel, external_id), UNIQUE(workspace_id, id)
);
CREATE INDEX leads_workspace_date ON leads(workspace_id, created_at DESC);
CREATE TABLE outcomes (
    workspace_id uuid NOT NULL, lead_id uuid NOT NULL,
    external_id text NOT NULL, amount_minor bigint NOT NULL CHECK (amount_minor >= 0),
    currency text NOT NULL CHECK(currency = 'USD'),
    status text NOT NULL CHECK(status IN ('booked','canceled')),
    revision integer NOT NULL CHECK(revision > 0),
    PRIMARY KEY(workspace_id, external_id),
    FOREIGN KEY(workspace_id, lead_id) REFERENCES leads(workspace_id,id)
);
CREATE TABLE webhook_receipts (
    workspace_id uuid NOT NULL REFERENCES workspaces(id), fingerprint text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(workspace_id,fingerprint)
);
CREATE TABLE audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    workspace_id uuid NOT NULL, lead_id uuid NOT NULL,
    kind text NOT NULL, payload jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(workspace_id,lead_id) REFERENCES leads(workspace_id,id)
);
