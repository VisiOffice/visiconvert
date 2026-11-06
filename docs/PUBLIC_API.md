# Workspace API keys and public API v1

Deployed on October 23, 2026 for hosted workspaces and the standalone pilot.
Self-hosted installations can use the same feature by upgrading to the
[patched source revision](https://github.com/VisiOffice/visiconvert/tree/6b76432bc1fde9686266cd5ec03449235d2a9685).
Upgrade the engine and CLI/MCP together.

## Create a key

Open your workspace as its owner (hosted) or operator (self-hosted), then **API
keys**. Enter a descriptive name, choose optional write permissions and an optional
UTC expiration. Read access is included and exposes customer contacts and notes.
Save the secret when shown: it is displayed once and only its SHA-256 hash is
stored. The secret is not retrievable. If a create/rotate response is lost, inspect
the key list, revoke the inaccessible key, and create another; creation is not an
idempotent operation.

Use one key per integration. Up to 50 active keys are allowed per workspace.
The UI lists up to 200 keys, with active keys first, and the newest 100 management events. Name, prefix, scopes,
expiration, creation, last-use and revocation are visible; no full secret/hash is
included in metadata. Last-used means an otherwise-valid key was presented, even
if the request subsequently failed a permission or hosted billing check.

Permissions:

| Scope | Operations |
|---|---|
| `read` | Workspace, inquiries, interactions, bookings, source reports, numbers, Twilio and SignalWire status/inventory, setup evidence and site/DNI settings |
| `leads:write` | Create form interactions; update shared inquiry qualification/notes |
| `bookings:write` | Record, revise, cancel bookings |
| `numbers:write` | Register an existing voice number in the connected Twilio or SignalWire account |
| `numbers:purchase` | Search, buy, and release SignalWire numbers (`/numbers/search`, `/numbers/purchase`, `/numbers/{id}/release`, `/numbers/{id}/restore`). Billable: each number is charged monthly until released. Owners grant it under **Billable permission** in API Keys; the expiration defaults to 24 hours. |
| `setup:write` | Create/update website settings and DNI pools (engine migration 0012) |
| `conversions:write` | Define and map pipeline conversion actions (`PUT /conversion-actions/{key}`, `POST /conversion-actions/{key}/google-action`) and post pipeline conversion events (`POST /conversion-events`) that VisiConvert matches and sends to Google Ads (engine migration 0023). It also maps and creates Microsoft goals and runs the Microsoft check (migration 0027) |
| `conversions:send` | Choose the advertising account and whether inferred calls are sent (`PUT /conversion-setup/destination`), and turn Google delivery and automatic sending on or off and set the consent policy (`PUT /conversion-delivery`), on the owner's behalf. Attestations are recorded for the owner who created the key. It also makes a Google Ads conversion action primary or secondary (`PUT /conversion-setup/google-actions/{id}/goal`), and turns an account-level conversion goal on or off for bidding (`PUT /conversion-setup/google-goal`), which changes what Smart Bidding optimizes for across the account. Conversions sent to Google cannot be retracted. It also saves the Microsoft Advertising account (`PUT /conversion-setup/microsoft-destination`). Owners grant it under **Sending permission** in API Keys; the expiration defaults to 24 hours (engine migration 0024). |

Keys cannot manage keys, billing, provider credentials, inquiry grouping,
reviewed conversion preparation, or the Google connection. Google delivery, automatic sending and the consent policy need `conversions:send`. Use the workspace UI for the rest. Read-only
keys remain read-only even if an MCP client enables change tools.

Rotate creates a replacement and revokes the old key atomically, preserving the
permissions/expiration selected in the UI. To overlap credentials during a rollout,
create a separate key, update the integration, then revoke the old key. Expired
keys should be replaced with a new key and future expiration. Rotation/revocation
use optimistic versions; stale requests fail with 409.

Revoked/expired keys fail authentication on subsequent requests; in-flight requests
may finish. Hosted keys are bound to their creating owner and credential version.
Membership/workspace disablement or an owner password reset invalidates access.
Every hosted key request also asks Rails to verify ownership, membership, email
verification, credential version, workspace readiness and paid access. Billing
checks reflect Rails' current recorded subscription state; normal Stripe webhook
processing still applies. Rails is required for hosted API availability: a failed
check returns 503, without a grace period. Self-hosted keys have no Rails dependency.

## Authentication

Base URL: `https://workspaces.visiconvert.com/api/v1` for hosted workspaces; your own engine origin plus `/api/v1` for self-hosting. The hosted billing
site `cloud.visiconvert.com` is not the engine API.

```sh
export VISICONVERT_URL=https://workspaces.visiconvert.com
# Load VISICONVERT_API_KEY privately from your secret manager.
curl -H "Authorization: Bearer $VISICONVERT_API_KEY" \
  "$VISICONVERT_URL/api/v1/inquiries?limit=50"
visiconvert login --url "$VISICONVERT_URL"
# Paste the workspace API key into the hidden prompt.
visiconvert status --json
visiconvert mcp
```

`vc_key_…` credentials select v1 in the updated CLI/MCP. Hosted browser session
credentials are not accepted by v1. Existing self-hosted operator keys still work
with the legacy CLI/private API; those are unrestricted administration credentials,
not recommended integration keys. The CLI's legacy mode retains the server's
500-interaction listing cap. With a new scoped key it follows v1 pagination and
can retrieve a full interaction UUID directly. Large automatic scans can hit the
per-key rate limit; use filtered direct API requests for bulk integrations.

The shared CLI client requires an HTTPS origin, permitting HTTP only for exact
loopback hosts. It rejects URL credentials, paths, queries, fragments, and redirects.
This applies to login, saved configuration and environment overrides. Never put
keys in query strings, browser-side website code, source control, or agent chat.

## Contract, pagination and errors

The machine-readable contract is `GET /api/v1/openapi.json`, which needs no key.
Its source is `crates/convert-server/assets/openapi.json`; regenerate with
`python3 scripts/generate-openapi.py` and verify using `--check`.

`/interactions` (also `/leads`), `/inquiries`, and `/bookings` return
`{"data":[…],"next_cursor":null}`. Default page size is 50, maximum 100. Follow a
non-null `next_cursor` by URL-encoding it as `cursor`, preserving other filters.
Interactions/inquiries use descending creation time and UUID; bookings use
ascending external reference. Paging is not a frozen database snapshot: updates
and newly captured records can change results between requests. Newer records
require a fresh scan from the first page.

Interaction/inquiry filters: `since` (RFC3339 timestamp), `source` (case-insensitive
exact match), `qualification` (unreviewed/qualified/unqualified/spam). Bookings
accept only `limit` and `cursor`. Numbers and source reports return arrays. Twilio
inventory returns one provider page with `has_more`; it is not a complete export.

`GET /activity` (read) is one newest-first timeline: calls, forms, pipeline conversion
events and their Google and Microsoft deliveries, webhook deliveries with their last HTTP
status, and configuration changes made through the UI, API keys or agents with who made
them. Filter with `kind` (call, form, conversion, webhook or api) and `since`; page with
`limit` and `cursor` like the lists above. Items are summaries built from fields other
endpoints already return: no form payloads, webhook payloads, provider requests, click IDs
or secrets. The MCP tool `list_activity` reads the same log.

Each scoped key permits 120 requests per one-minute window. A 429 includes
`Retry-After: 60`. Missing/expired/revoked keys return 401, missing permissions or
hosted access return 403, missing workspace-owned resources return 404, conflicting
writes return 409, and hosted verification outages return 503. Validation may
return 400 or 422. Requests are limited to 64 KiB. v1 errors use `{"error":"…"}`.
The UI/legacy private API is not the versioned integration contract.

## Write semantics

`POST /api/v1/interactions` requires `external_id`, `name`, `contact`, `source`,
`campaign`, and `notes`; empty campaign/notes are permitted. An identical payload
with the same workspace external ID returns the original ID. A changed payload
with that ID conflicts. CLI/MCP can generate an ID, but integrations should supply
one stable ID per submission for retry safety.

Qualification, notes and version belong to the inquiry. Reviewing any member
interaction reviews its entire inquiry. Send the current version; on 409 re-read
and reconcile instead of overwriting. Public v1 does not expose combine/split.

Booking references are workspace-unique. Use exact integer USD cents and revision
1 for a new reference, then increment by one. An identical revision/payload retry
is accepted without extra revenue. Changed stale revisions fail. Bookings remain
attached to their original interaction across grouping, and can be revised through
another member of the same inquiry using the proper revision. Use inquiry outcomes
or the paginated workspace booking list to find them. An interaction's outcomes
list contains only its original bookings. CLI/MCP currently address original
interaction outcomes and compute the next revision; repeated successful CLI calls
may add a revision but never a duplicate booking.

Reports count active inquiries, not individual interactions, and attribute grouped
revenue to the primary interaction's source. They are all-time and include demo
records; interaction listings include demo records as well. CLI list filtering
hides demo records by default. Captured source is attribution evidence, not proof
of ad causation. Google conversion delivery is not enabled by these API keys.

## Hosted rollout

Deploy Rails' service-authenticated `POST /internal/api-access` before the engine.
It accepts only the existing separate cloud service credential and identity IDs;
it never receives public API-key secrets. Keep the service key independent from
operator and integration keys. Verify active, unpaid, reset, disabled-membership,
and Rails-unavailable paths before enabling customer integrations. Key issuance
checks entitlement; listing and revocation remain available to an authenticated
owner so unwanted keys can be removed. No new Stripe products or billing behavior
are introduced.

Migration 0009 is additive. Existing database backups include hashes/metadata and
key audit history, but not plaintext integration secrets. Protect backups: restoring
an older database can restore keys revoked after that backup; revoke affected keys
before exposing a restored installation. Keep integration secrets in a separate
secret manager. Existing operator/bootstrap access is unchanged.

## Pipeline conversions

Keys with `conversions:write` can define actions (`PUT /api/v1/conversion-actions/{key}`),
map them to existing Google actions found with `GET /api/v1/connections/google/actions`,
create a new VisiConvert action in Google for one (`POST /api/v1/conversion-actions/{key}/google-action`),
and post business events to `POST /api/v1/conversion-events`. VisiConvert matches the
lead and its Google click and sends the conversion when Google delivery and automatic
sending are on. Reads (`GET /api/v1/conversion-setup`, `/conversion-delivery`,
`/conversion-actions`, `/conversion-events`, `/connections/google/accounts`) need `read`.
Keys with `conversions:send` can switch delivery and automatic sending and set the
consent policy with `PUT /api/v1/conversion-delivery`; setting
`not_required_outside_eea` needs `attest_no_eea_advertising: true`, recorded for the
owner who created the key. `conversions:write` can also run the read-only destination check
(`POST /api/v1/conversion-setup/check`). `conversions:send` can also save the advertising
account and inferred-call opt-in (`PUT /api/v1/conversion-setup/destination`).
`conversion-setup` says which steps are left; no key can connect Google. See [pipeline conversions](PIPELINE_CONVERSIONS.md).

Microsoft Advertising uses the same scopes. `read`: `GET /api/v1/connections/microsoft/accounts`,
`GET /api/v1/connections/microsoft/goals` and `GET /api/v1/conversion-setup/microsoft-review`.
`conversions:write`: map an action to a goal (`PUT /api/v1/conversion-actions/{key}/microsoft-goal`),
create a goal for it (`POST` on the same path) and run the read-only check
(`POST /api/v1/conversion-setup/microsoft-check`). `conversions:send`: save the ad account
(`PUT /api/v1/conversion-setup/microsoft-destination`). The `microsoft` section of
`conversion-setup` lists the steps left; no key can connect Microsoft. See
[Microsoft connection](MICROSOFT_CONNECTION.md).

## Webhooks

Keys with `webhooks:write` can create and manage signed outbound webhook endpoints
under `/api/v1/webhooks`; listing endpoints and deliveries needs `read`. See
[webhooks](WEBHOOKS.md) for events, signatures and retry behavior.

## SignalWire

Read project status and callback readiness at `GET /api/v1/connections/signalwire`, and owned voice-number inventory at `GET /api/v1/connections/signalwire/numbers`. Both require `read`. Register a number with `POST /api/v1/numbers`, `numbers:write`, and `provider: "signalwire"`. Omitted provider remains `twilio`. Provider secrets are configured only in the workspace UI. See [SignalWire setup](SIGNALWIRE_CONNECTIONS.md).

## Agent onboarding and website setup

With engine migration 0012, `GET /api/v1/setup` returns per-site tracker/form activity,
DNI configuration and assignment evidence, and per-number call receipts. This is
historical evidence, not proof of current forwarding or correct attribution.
`GET /api/v1/sites` lists settings, current versions and the public tracker URL.
`GET /api/v1/sites/{id}/number-pool` returns pool configuration and recent assignments.
These reads require `read`.

`setup:write` grants `POST /api/v1/sites`, `PATCH /api/v1/sites/{id}` and
`PUT /api/v1/sites/{id}/number-pool`. Existing keys do not gain this permission.
Use version 0 for a new site/pool; use the current version for updates. Creation
is not retry-safe: inspect the site list after an uncertain response. A 409 on
update requires reviewing the current settings before trying again. Pausing a site
or changing its origin revokes active DNI assignments. Pool replacement can revoke
assignments for removed numbers. No operation purchases numbers or changes carrier routing.

The **Setup** screen provides a key preset (read + setup:write, seven-day expiry),
a local stdio MCP configuration and a guided prompt. Registering owned numbers
also requires `numbers:write`. Provider credentials remain in Connections; never
send secrets to agent chat. Revoke temporary setup keys after acceptance checks.

### Website pool phone fields

`PUT /api/v1/sites/{id}/number-pool` accepts `fallback_phone` (the published website number to replace) and optional `destination_phone` (where pool calls should ring), both in +1 E.164 format. They may differ. Every selected number must already forward to `destination_phone`; pool saves do not modify carrier routing. Keep the published number outside all pools. A registered static fallback must belong to this workspace and share the destination.

Send `destination_phone` explicitly for new integrations. Omitting it on an update preserves the existing destination; on creation it defaults to `fallback_phone` for older clients. Changing either value revokes active assignments without shortening their cooldown. Public tracker responses do not include the forwarding destination.

For Google Business Profile, register a static number through the existing numbers API with source `Google Business Profile` and campaign naming the location. Keep it outside all website pools. Listing/phone changes in Google and carrier routing remain manual; this does not enable Google Ads uploads.
