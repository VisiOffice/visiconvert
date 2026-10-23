---
title: API keys and public API v1
description: Create scoped workspace API keys and integrate with the versioned HTTP API.
---

Available in the current source for hosted and self-hosted workspaces. Deployment
and public source publication are pending for this slice. The engine, CLI/MCP,
and hosted Rails application must be upgraded before using it.

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
| `read` | Workspace, inquiries, interactions, bookings, source reports, numbers, Twilio status/inventory |
| `leads:write` | Create form interactions; update shared inquiry qualification/notes |
| `bookings:write` | Record, revise, cancel bookings |
| `numbers:write` | Register an existing voice number in the connected Twilio account |

Keys cannot manage keys, billing, Twilio credentials, sites, DNI, inquiry grouping,
or conversion preparation. Use the workspace UI for those operations. Read-only
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

Base URL: `https://workspaces.visiconvert.com/api/v1` for hosted workspaces after
rollout; your own engine origin plus `/api/v1` for self-hosting. The hosted billing
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
