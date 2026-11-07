# Meta Ads connection

Meta (Facebook and Instagram ads) is a third destination for [pipeline
conversions](PIPELINE_CONVERSIONS.md), next to Google and Microsoft. VisiConvert sends
each eligible event to your Meta dataset (Pixel) through the Conversions API, with the
customer's phone and email hashed, and the Meta click and browser IDs the website tracker
captured. It works the same in hosted and self-hosted workspaces and needs engine
migration 0029.

Saving the connection sends nothing. Events are sent only when all of these hold: the
dataset and access token are saved, the action is mapped to a Meta event, and the shared
delivery switch and automatic sending are both on.

> **Status.** The Conversions API calls follow Meta's published reference and are tested
> against a local mock only. Treat the first live test event as the acceptance test.

## What you need

- A Meta dataset (Pixel) in Events Manager, ideally the one your website's Pixel already
  uses, so Meta can deduplicate and match.
- A **Conversions API access token** for that dataset: Events Manager → the dataset →
  **Settings** → **Conversions API** → **Generate access token**. No Meta app review or
  business verification is needed for this token.

The token is sealed with the installation's encryption key (`VISICONVERT_ENCRYPTION_KEY`)
and never shown again; VisiConvert shows only its first 12 SHA-256 hex characters so you can
tell tokens apart. Generate a new one in Events Manager to rotate it, and save it the same
way.

## Connect

**In the app:** Conversions → Meta Ads, paste the dataset ID and token, and save.
VisiConvert reads the dataset with the token first and refuses a token Meta rejects.

**From the CLI** with a key that has `conversions:send`, reading the token from an
environment variable so it never appears in a command line or chat:

```bash
META_CAPI_TOKEN=... visiconvert connect meta --dataset 1234567890123 --token-env META_CAPI_TOKEN
```

`--token-stdin` reads it from standard input instead. The API is
`PUT /api/v1/connections/meta` with `{revision, dataset_id, access_token}` (`revision` is 0
before the first save; `meta.revision` in `GET /api/v1/conversion-setup` after).
`DELETE /api/v1/connections/meta` with `{revision}` disconnects.

No MCP tool accepts the token. An agent asks the owner to save it, then does the rest.

## Map actions to Meta events

Each pipeline action is sent under a Meta event name: a standard event such as `Lead` (new
leads), `Schedule` (booked appointments) or `Purchase` (won jobs, which needs a value), or a
custom name (a letter, then up to 39 letters, digits or underscores).

- MCP: `set_meta_action_event` with `{key, version, event_name}` (`conversions:write`);
  `event_name: null` clears it.
- API: `PUT /api/v1/conversion-actions/{key}/meta-event`.

A new or changed mapping applies to events recorded from then on, so another integration
already sending the same events cannot be counted twice. To also send events from the last 7
days, pass `include_since`, but only when nothing else sent those events to Meta.

## Check, and send a test event

`check_meta_destination` (`POST /api/v1/conversion-setup/meta-check`, `conversions:write`)
reads the dataset with the saved token and records the result. Open Events Manager →
the dataset → **Test events**, copy the test code, and pass it as `test_event_code`:
VisiConvert sends one `Lead` test event there. Test events never count as conversions.
VisiConvert repeats the read-only check by itself while events are waiting.

## Which leads are sent

`set_meta_settings` (`PUT /api/v1/conversion-setup/meta-settings`, `conversions:send`)
takes `{settings_version, match, ldu}`. Both choices change what is shared with Meta, so
an agent asks the owner first.

- `match: meta_clicks` (default) sends only leads whose website visit had a Meta ad click:
  an `fbclid`, or a Meta campaign tag (`utm_source` facebook, fb, instagram, ig or meta)
  with the Pixel's `_fbp` browser ID.
- `match: all_leads` sends every eligible lead, including calls and leads with no website
  visit, and Meta matches them by hashed phone and email.
- `ldu: geolocate` marks events for Meta's
  [Limited Data Use](https://developers.facebook.com/docs/marketing-apis/data-processing-options),
  so Meta applies US state privacy laws by location. With a visitor IP address Meta
  geolocates it; without one, events for California phone numbers are marked for
  California. `ldu: off` sends no data processing options.

## What is sent

One event per request, built when it is planned:

| Field | Value |
| --- | --- |
| `event_name` | The mapped name |
| `event_time` | When the event happened |
| `event_id` | `vc-<event id>`, so a resend or the browser Pixel's copy is deduplicated |
| `action_source` | `website` for new form leads, `phone_call` for new calls, `system_generated` for events your backend posts (quotes, orders); `other` when a website lead has no browser details |
| `user_data.ph`, `em` | SHA-256 of the normalized phone (with country code) and email |
| `user_data.external_id` | SHA-256 of the VisiConvert lead ID |
| `user_data.fbc`, `fbp` | The `_fbc` cookie or one built from the `fbclid`, and the `_fbp` cookie |
| `user_data.client_ip_address`, `client_user_agent` | The visitor's, when kept (below) |
| `custom_data` | `value` and `currency` (USD) when the event has a value |

Raw phone numbers and email addresses are never stored in delivery records or sent.

## Browser details

Meta matches website events better with the visitor's user agent and IP address. The
tracker sends the Pixel's `_fbp` and `_fbc` cookies with the visit when present, and the
engine keeps the user agent and IP address **only for workspaces with a Meta connection**.
They are cleared after 30 days and when Meta is disconnected.

The IP address is taken from the header your reverse proxy sets, named by
`VISICONVERT_CLIENT_IP_HEADER` (for example `x-forwarded-for`, whose last entry is the one
the proxy added). Without that setting, no IP address is kept. Set it only when every
request reaches the engine through that proxy.

## Rules Meta imposes

- **No changes after sending.** Meta cannot restate or withdraw an event. If an event's
  value changes or it is canceled after it was sent, the first version stands, and the
  event says why.
- **7 days.** Meta rejects events more than 7 days old. Events older than that are never
  sent, and the event says so.
- **Uncertain sends are not repeated.** If VisiConvert cannot tell whether Meta received
  an event (a timeout or an unreadable reply), it is marked uncertain and never resent
  automatically; check Events Manager.
- **Same holds as Google.** New leads wait 6 hours for review; spam and unqualified
  leads, missing consent (under the per-lead policy) and DNI calls matched by timing
  without the owner's opt-in are held.

## Turn off other senders

If a CRM, form tool or website plugin already sends the same events to this dataset
through the Conversions API, turn it off for those events, or Meta counts them twice.
The browser Pixel's own `Lead` event is fine when it uses the same `event_id`; otherwise
map VisiConvert to a different event name or turn the Pixel event off.

## Status and webhooks

Each pipeline event shows `delivery.meta`: `pending`, `blocked` (with the reason),
`queued`, `delivered`, `uncertain`, `failed` or `canceled`, and Meta's `trace_id` for a
delivered event. `conversion.delivered` and `conversion.failed` webhooks fire for Meta
deliveries with `provider: "meta"`. The `meta` section of `GET /api/v1/conversion-setup`
lists what is left to do.
