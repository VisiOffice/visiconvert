# Webhooks

VisiConvert can send signed HTTPS requests to your CRM, Zapier, Make, n8n or your own
service when leads and bookings change. Webhooks work the same in hosted and
self-hosted workspaces and need engine migration 0022 and `VISICONVERT_ENCRYPTION_KEY`.

## Events

| Type | Sent when |
|---|---|
| `interaction.created` | A call or form lead is recorded |
| `call.completed` | A call finishes, with its status and duration |
| `inquiry.reviewed` | An inquiry's qualification changes, including to `spam` |
| `booking.recorded` | A booking is first recorded |
| `booking.revised` | A booking's amount or revision changes |
| `booking.canceled` | A booking is canceled |
| `conversion.delivered` | Google Ads accepted a conversion |
| `conversion.failed` | A Google Ads conversion failed or needs review |
| `webhook.test` | You send a test from the endpoint |

Demo data never sends events. Combining or separating inquiries and notes-only edits
do not send events.

## Create an endpoint

Owners can manage endpoints in the workspace, or use the API with a key that has the
`webhooks:write` permission (listing needs only `read`):

```sh
curl -X POST "$VISICONVERT_URL/api/v1/webhooks" \
  -H "Authorization: Bearer $VISICONVERT_KEY" -H "Content-Type: application/json" \
  -d '{"url":"https://crm.example.com/hooks/visiconvert","description":"CRM",
       "event_types":["interaction.created","inquiry.reviewed","booking.recorded"]}'
```

The response contains a `whsec_…` secret. **It is shown once**; store it in your
receiver's secret manager. A workspace can have up to 10 endpoints.

URLs must use HTTPS on port 443 with a public hostname. Requests are never sent to
private, loopback, link-local or other reserved addresses, and redirects are not
followed. A self-hosted operator can set `VISICONVERT_WEBHOOKS_ALLOW_PRIVATE=true` to
allow private addresses and plain HTTP, for example an n8n container on the same
network. Hosted workspaces ignore that setting.

Other endpoints: `GET/PATCH/DELETE /api/v1/webhooks/{id}`,
`POST /api/v1/webhooks/{id}/rotate-secret`, `POST /api/v1/webhooks/{id}/test`,
`GET /api/v1/webhooks/{id}/deliveries` and
`POST /api/v1/webhook-deliveries/{id}/redeliver`. See the OpenAPI document at
`/api/v1/openapi.json`. Updates replace all settings and require the current `version`.

## Payload

```json
{
  "id": "1c7e…",
  "type": "booking.revised",
  "occurred_at": "2026-11-02T18:21:52Z",
  "workspace_id": "d3f1…",
  "resource_version": 2,
  "data": { "object": "booking", "external_id": "job-1", "amount_minor": 65000, "...": "…" }
}
```

`data` uses the same fields as the matching `/api/v1` resource (interaction, inquiry,
booking) plus `object`. Interactions and inquiries also include `fields`, the form
fields your site was allowed to collect. The payload is built when the event is first
sent and then kept, so retries carry identical bytes. Use `occurred_at` and
`resource_version` to ignore stale or out-of-order events.

Set `include_contact` to `false` to leave out `name`, `contact`, `notes` and `fields`;
fetch them through the API with a scoped key when needed.

## Verify signatures

Requests follow the [Standard Webhooks](https://www.standardwebhooks.com/) spec, so
its libraries work unchanged. Each request has `webhook-id`, `webhook-timestamp` and
`webhook-signature` headers. The signature is `v1,` followed by the base64
HMAC-SHA256 of `{webhook-id}.{webhook-timestamp}.{raw body}`, keyed with the base64
decoded part of the secret after `whsec_`.

```js
import { Webhook } from "standardwebhooks";
const wh = new Webhook(process.env.VISICONVERT_WEBHOOK_SECRET.replace("whsec_", ""));
// Pass the raw request body, not re-serialized JSON.
const event = wh.verify(rawBody, request.headers);
```

Without a library:

```js
import crypto from "node:crypto";
function verify(secret, headers, rawBody) {
  const id = headers["webhook-id"], ts = headers["webhook-timestamp"];
  if (Math.abs(Date.now() / 1000 - Number(ts)) > 300) return false;
  const key = Buffer.from(secret.replace("whsec_", ""), "base64");
  const expected = "v1," + crypto.createHmac("sha256", key).update(`${id}.${ts}.${rawBody}`).digest("base64");
  return headers["webhook-signature"].split(" ").some(s =>
    s.length === expected.length && crypto.timingSafeEqual(Buffer.from(s), Buffer.from(expected)));
}
```

Reject timestamps more than five minutes old. After **Rotate secret**, requests carry
signatures from both the old and new secret for 24 hours, separated by a space.

## Delivery

- Any 2xx response within 10 seconds is success.
- Timeouts, connection errors, 408, 429 and 5xx are retried after 5 seconds,
  5 minutes, 30 minutes, 2, 5, 10 and 10 hours (about 28 hours), then marked failed.
  `Retry-After` is honored up to one hour.
- Other 4xx responses fail without retry. 410 Gone also disables the endpoint.
- If every delivery to an endpoint fails for 5 days, the endpoint is disabled and its
  queued deliveries are canceled. Re-enable it by updating its `state` to `enabled`.
- Delivery is at least once and not ordered. Deduplicate on `webhook-id`.
- Pausing an endpoint holds its deliveries until it is resumed.
- The delivery log keeps status codes and errors, never response bodies, for 30 days.
  Redelivering resends the original payload with the same `webhook-id`.

Endpoint changes, rotations, redeliveries and automatic disabling are recorded in the
endpoint history.
