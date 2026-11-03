# Outbound webhooks — design

Status: **implemented** (migration 0022, backend and web app Webhooks screen). Target: one shared Rust feature for hosted and
self-hosted workspaces, like Google delivery. No Rails, billing, or VisiComms dependency.

## Why

Integrations are the most-cited complaint in competitor reviews (CallRail's top G2 con;
WhatConverts CRM sync complaints). Today VisiConvert has a pull-only REST API. Signed push
events let a CRM, Zapier, Make, or n8n react to leads and bookings without polling, and give
self-hosters a stable integration surface that does not depend on a connector catalogue.

## Goals and non-goals

**v1 does:** per-workspace HTTPS endpoints; a small set of signed events; at-least-once delivery
with retries; a delivery log with manual redelivery; management in the web app and public API.

**v1 does not:** native CRM connectors, payload templates or filters beyond event type,
ordering guarantees, inbound webhooks, MCP/CLI management, or a Zapier app (that comes later
and uses the same API).

## Events

All events are workspace-scoped. Demo leads (`is_demo`) never produce events.

| Type | Fires when | Source of truth |
|---|---|---|
| `interaction.created` | A call or form lead is inserted | `AFTER INSERT ON leads` |
| `call.completed` | A call reaches a terminal state (`dial_terminal` false → true), with duration and status | `AFTER UPDATE ON leads` |
| `inquiry.reviewed` | An inquiry's qualification changes, including to `spam` | `AFTER UPDATE ON inquiries` when `qualification` changes |
| `booking.recorded` | A booking is first recorded | `AFTER INSERT ON outcomes` |
| `booking.revised` | Amount changes on an existing booking (`revision` increases, still `booked`) | `AFTER UPDATE ON outcomes` |
| `booking.canceled` | Status changes to `canceled` | `AFTER UPDATE ON outcomes` |
| `conversion.delivered` | A Google delivery reaches `processed` | `AFTER UPDATE ON conversion_deliveries` |
| `conversion.failed` | A Google delivery reaches `failed` or `needs_review` | same |
| `webhook.test` | Sent from **Send test** only | API |

Every qualification write already goes through `update_inquiry_tx` (the legacy lead PATCH
included), so one trigger on `inquiries` covers all paths. Inquiry combine/split and notes-only
edits do not emit events in v1.

## Capture: transactional outbox via triggers

Events are recorded in the same transaction as the change that caused them, using `AFTER`
triggers — the pattern already used by conversion preparation (0008) and delivery
invalidation (0020). No write path in Rust has to remember to emit, and nothing is lost if the
process crashes after commit.

Triggers write only when the workspace has at least one enabled endpoint subscribed to that
type, so workspaces without webhooks pay one indexed `EXISTS` and store nothing.

```sql
-- migration 0022_webhooks.sql (sketch)
CREATE TABLE webhook_endpoints (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces(id),
  url text NOT NULL CHECK (url LIKE 'https://%' OR url LIKE 'http://%'),
  description text NOT NULL DEFAULT '',
  event_types text[] NOT NULL,              -- subscribed types
  include_contact boolean NOT NULL DEFAULT true,
  encrypted_secret text NOT NULL,           -- sealed with the existing Vault
  previous_encrypted_secret text,           -- still signed with during rotation
  previous_secret_expires_at timestamptz,
  state text NOT NULL DEFAULT 'enabled' CHECK (state IN ('enabled','paused','disabled')),
  disabled_reason text,
  version integer NOT NULL DEFAULT 1 CHECK (version > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, id)
);
CREATE TABLE webhook_events (
  id uuid PRIMARY KEY,                      -- the public event id (idempotency key)
  workspace_id uuid NOT NULL REFERENCES workspaces(id),
  type text NOT NULL,
  resource_kind text NOT NULL,              -- 'interaction' | 'inquiry' | 'booking' | 'conversion'
  resource_id text NOT NULL,
  resource_version integer,
  occurred_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webhook_deliveries (
  id uuid PRIMARY KEY,
  workspace_id uuid NOT NULL,
  event_id uuid NOT NULL REFERENCES webhook_events(id),
  endpoint_id uuid NOT NULL,
  payload jsonb,                            -- frozen on first attempt; retries resend identical bytes
  state text NOT NULL CHECK (state IN ('pending','sending','retry','succeeded','failed','canceled')),
  attempts integer NOT NULL DEFAULT 0,
  due_at timestamptz,
  last_status integer,
  last_error text,                          -- bounded; never the response body
  last_attempt_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (workspace_id, endpoint_id) REFERENCES webhook_endpoints(workspace_id, id),
  UNIQUE (event_id, endpoint_id)
);
CREATE INDEX webhook_delivery_due ON webhook_deliveries(due_at) WHERE state IN ('pending','retry','sending');
```

The trigger function inserts one `webhook_events` row and one `webhook_deliveries` row per
matching enabled endpoint, then `pg_notify('webhooks', '')` so the worker wakes immediately.

## Payload

Payloads are rendered in Rust by the worker on the **first attempt**, using the same
serializers as `/api/v1` (so a webhook body matches what `GET /api/v1/...` would return), then
frozen in `webhook_deliveries.payload`. Retries and redeliveries send identical bytes.

```json
{
  "id": "evt_7c1f…",
  "type": "booking.revised",
  "occurred_at": "2026-11-02T18:21:52Z",
  "workspace_id": "d3f1…",
  "data": {
    "object": "booking",
    "version": 3,
    "...": "the /api/v1 representation at render time"
  }
}
```

- `data` reflects the resource **when the event is first sent**, not strictly when it occurred;
  `occurred_at` and `version` let receivers drop stale or out-of-order events.
- With `include_contact = false`, phone numbers, emails, names and form fields are omitted and
  receivers fetch them through the API with a scoped key.
- A resource deleted or made inaccessible before rendering produces `data: null` with the ids.

## Signing

Follow the [Standard Webhooks](https://www.standardwebhooks.com/) spec so receivers can use
existing libraries:

- Headers `webhook-id` (event id), `webhook-timestamp` (unix seconds), `webhook-signature`
  (`v1,<base64 HMAC-SHA256 of "id.timestamp.body">`).
- Secret is 32 random bytes shown once as `whsec_…`, stored sealed with the existing `Vault`.
- **Rotation**: a new secret is issued; the previous one keeps signing for 24 hours, and both
  signatures are sent space-separated, so receivers can switch without dropping events.
- Docs include verification snippets (Node, Python, Ruby, PHP) and the 5-minute timestamp
  tolerance receivers should enforce.

## Delivery worker

Same shape as `delivery::spawn`: one background task per server, claims with
`FOR UPDATE SKIP LOCKED`, leases rows by moving them to `sending` with a 2-minute `due_at`, and
recovers expired leases on the next claim. It wakes on `LISTEN webhooks` with a 10-second
fallback tick.

- Up to 8 concurrent requests per server, at most 2 in flight per endpoint.
- 10-second total timeout. Any 2xx is success. Redirects are not followed (as with the Google
  client). Response bodies are not stored.
- Retry schedule after a failure (timeout, connection error, 408, 429, 5xx): 5 s, 5 min,
  30 min, 2 h, 5 h, 10 h, 10 h — about 28 hours, then `failed`. Other 4xx fail immediately
  except 410 Gone, which also disables the endpoint. `Retry-After` is honoured up to 1 hour.
- **Auto-disable**: an endpoint whose deliveries have all failed for 5 consecutive days moves to
  `disabled` with a reason shown in the app; pending deliveries are `canceled`. Re-enabling
  does not replay them (manual redelivery remains available).
- Delivery is at-least-once. Receivers must dedupe on `webhook-id`.

## Network safety (SSRF)

Endpoint URLs are user-supplied, so the hosted service must not become a proxy into its own
network.

- **Hosted:** `https://` only, port 443. At send time, resolve the host, reject loopback,
  private (RFC 1918), link-local (incl. 169.254.169.254), CGNAT, multicast, unique-local and
  IPv4-mapped IPv6 variants of those, then connect to the **checked address** (reqwest
  `resolve` override) so DNS rebinding cannot swap it. Checked again on every attempt, not
  only on save. Literal IPs in URLs are rejected.
- **Self-hosted:** same defaults. `VISICONVERT_WEBHOOKS_ALLOW_PRIVATE=true` permits private
  addresses and `http://` for operators posting to an internal CRM or n8n.
- URLs with credentials (`user:pass@`) are rejected; secrets belong in the signature.
- Per workspace: at most 10 endpoints.

## Management

**Web app** (owners only, under Settings → Webhooks): add an endpoint (URL, event checkboxes,
include contact details), copy the secret once, send a test, pause/resume, rotate the secret,
delete, and a delivery log per endpoint (event, status, attempts, last status code, next
retry) with **Redeliver**.

**Public API** with a new scope `webhooks:write` (reads need `read`):

```
GET    /api/v1/webhooks
POST   /api/v1/webhooks                      -> includes secret once
PATCH  /api/v1/webhooks/{id}                 (url, event_types, include_contact, state; version-checked)
DELETE /api/v1/webhooks/{id}
POST   /api/v1/webhooks/{id}/rotate-secret   -> new secret once
POST   /api/v1/webhooks/{id}/test
GET    /api/v1/webhooks/{id}/deliveries      (cursor pagination)
POST   /api/v1/webhook-deliveries/{id}/redeliver
```

Endpoint changes are recorded in management history alongside API key events. Existing keys
do not gain `webhooks:write`.

## Retention

Events and deliveries are pruned after 30 days by the worker. Endpoints are kept until
deleted; deleting an endpoint cancels its pending deliveries.

## Testing

- Store tests: each trigger emits exactly the expected event once; demo leads and workspaces
  without endpoints emit nothing; `call.completed` fires once despite repeated status callbacks.
- Worker tests against a local receiver: signature verification with the reference library,
  retry schedule, `Retry-After`, 410 disable, lease recovery after a killed worker, identical
  bytes on retry, rotation dual-signing.
- SSRF tests: each blocked range, DNS rebinding (resolver returning a public then a private
  address), redirects not followed.
- Release verification: create an endpoint in a disposable install, record a booking, and
  receive a verified `booking.recorded`.

## Rollout

Additive migration 0022. Nothing fires until an endpoint is created, so upgrades change no
behaviour. Ship in two PRs: (1) schema, triggers, worker, API; (2) web app screens and docs
(guide page on docs.visiconvert.com plus generated reference). Then update the website
comparison row "API and webhooks" to In the beta.

## Decisions

1. **Contact details default:** on, with a per-endpoint `include_contact` toggle.
2. **Hosted egress IPs:** yes, publish fixed egress addresses for allowlisting. This is a
   hosted gateway change outside this repository; until it ships, the docs do not list addresses.
3. **Form fields:** included as `fields` (the stored submission) when `include_contact` is on.

## Implementation notes

- Demo leads are flagged just after insertion, so the worker re-checks `is_demo` before sending
  and cancels such deliveries without counting them as endpoint failures.
- A worker batch keeps claiming until nothing is due, since the two-in-flight cap per endpoint
  can stop a batch early.
