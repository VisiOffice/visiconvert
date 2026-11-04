## Unreleased — Several number pools per site

- A site can run several DNI pools. Each replaces one published website number (its swap
  target) with its own forwarding destination, so LA pages showing an LA line, Phoenix pages
  showing a Phoenix line and the rest of a site showing a toll-free number each swap from
  their own pool. A visit holds one number from each pool whose number is on the page.
- The tracker sends the published numbers marked on the page (`targets`); the server leases
  only from pools replacing them and answers `{status:"ok", assignments:[{replaces, ...}]}`.
  Requests without `targets` keep the single-pool response.
- `PUT /api/sites/{id}/number-pool` accepts `pool_id` or `new_pool`; `GET` adds `pools` and
  each number's `pool_id`. MCP `configure_site_pool` takes the same fields. The Sites screen
  adds a pool selector.
- A number can move between pools on its site after it is removed from its current pool.
- Migration 0026 gives existing pools an id; their numbers, assignments and versions are kept.

## Unreleased — Google click window fix

- Google's API returns int64 fields such as `clickThroughLookbackWindowDays` as strings.
  They were read as 0, which blocked every pipeline conversion as "not an enabled import
  action" and showed a 0-day click window in the destination check and review. Both forms
  are now read.

## Unreleased — Google Ads conversion review

- New `GET /api/v1/conversion-setup/google-review` (`read`) and MCP tool
  `review_google_conversion_setup`: every non-removed conversion action in the saved
  Google Ads account with its type, category, status, primary/secondary, counting, click
  window, value settings, owner and 30/90-day conversions, flagged in plain language:
  `mapped`, `active_other_uploader`, `dead_upload` and `call_overlap` (website call
  actions also note the conflict with number swapping). Calls from ads are not flagged.
  Results are saved (additive migration 0025).
- `get_conversion_setup` adds advisory steps (`"advisory": true`, never blocking `ready`):
  run the review, then review its warnings with the owner. The automatic-sending warning
  names actions the last review found still receiving uploads from another tool.
- New `PUT /api/v1/conversion-setup/google-actions/{id}/goal` `{primary, login?}` and MCP
  tool `set_google_action_goal`, requiring `conversions:send`: changes only
  `primary_for_goal` on an action owned by the saved account, records
  `google_action_goal_changed` with the key or connection, and returns 422 with a clear
  message when Google refuses.
- The review also reads `conversion_action.origin` and every `customer_conversion_goal`
  (category, origin, biddable). Each action reports `origin`, `goal`
  `{category, origin, biddable}`, `goal_shared_with` (other actions in the same goal) and
  `bidding` (primary and a biddable goal). `dead_upload`, `call_overlap` and the
  `active_other_uploader` warning now use `bidding`, so a primary action in a goal that is
  not used for bidding is no longer flagged.
- New `PUT /api/v1/conversion-setup/google-goal` `{category, origin, biddable, login?}`
  and MCP tool `set_google_goal_biddable`, requiring `conversions:send`: changes only
  `biddable` on one CustomerConversionGoal of the saved account
  (`customers/{id}/customerConversionGoals/{category}~{origin}`), records
  `google_goal_biddable_changed` with the goal and its actions under the key or
  connection, and never touches campaign-level goal overrides. This is how Google-hosted
  actions such as clicks to call stop counting for bidding.
- `set_google_action_goal` no longer sends Google-hosted actions to Google; it and any
  Google refusal return 422 naming the action's goal and every other action in it, for
  `set_google_goal_biddable`. It now returns only the changed action with the summary and
  warnings, not the full review.

## Unreleased — agent-chosen Google destination

- `PUT /api/v1/conversion-setup/destination`
  `{version, customer_id, allow_inferred_calls, qualified_action_id?, booking_action_id?}`
  and the MCP tool `set_google_destination` (both `conversions:send`) save the advertising
  account and inferred-call opt-in through the owner route's store path: the same
  validation, settings version, invalidation, and a `settings_saved` history row naming
  the key or MCP connection.
- The account must be reachable from the connected Google login and own its conversion
  tracking. Omitted reviewed-flow action IDs are kept for the same account; switching
  accounts requires stating them.
- `get_conversion_setup` reports `allow_inferred_calls`. Its account step is now
  `owner_approval`, and it adds an `owner_approval` inferred-call step when the
  workspace has DNI pools or inferred calls and the opt-in is off.

## Unreleased — agent-controlled conversion sending (`conversions:send`)

- New owner-granted `conversions:send` permission for API keys and MCP connections
  (additive migration 0024). With it, `PUT /api/v1/conversion-delivery`
  `{version, enabled?, auto_send?, consent_policy?, attest_no_eea_advertising?}` turns
  Google delivery and automatic sending on or off and sets the consent policy, using the
  owner routes' own rules and the shared delivery settings version.
  `GET /api/v1/conversion-delivery` needs only `read`. `conversions:write` alone is refused.
- `consent_policy: "not_required_outside_eea"` requires `attest_no_eea_advertising: true`.
  `attested_by` names the owner who created the key or approved the MCP connection; the
  conversion history row names the key or connection.
- While automatic sending is on, the response carries a `warning` to turn off other
  uploaders for the same Google actions, plus the setup checklist.
- New MCP tools `get_conversion_delivery` and `set_conversion_delivery` (requires
  `conversions:send`; the description tells agents to get the owner's explicit
  confirmation in chat and to state the attestation). Checklist steps an agent may take
  with the owner's go-ahead now have actor `owner_approval`.
- API Keys has a separate **Sending permission** checkbox with a notice; expiry defaults
  to 24 hours. The MCP approval page lists it apart, unchecked, with the same warning.
  Hosted remote MCP needs Rails to offer `conversions:write` and `conversions:send`.
- `conversions:write` can run the owner's read-only **Check saved connection**:
  `POST /api/v1/conversion-setup/check` `{revision, settings_version, login}` and MCP tool
  `check_google_destination`. Both use the owner route's code and record the same report.
  The checklist now includes `connection_revision`, and its first-check step is an
  `agent` step.
- Connecting Google and choosing the advertising account still need the owner.

## Unreleased — automatic pipeline conversions (Google)

- Workspaces define conversion actions (`lead`, `quote_sent`, `order`, …) with a trigger
  (posted events or each new attributed lead), once-per-click or every-event counting,
  an optional default value and a Google conversion action. Manage them under
  **Conversions**, the private `PUT /api/conversions/actions/{key}`, or with a key/MCP (below).
- `new_lead` conversions are held for 6 hours after the lead arrives and are not sent
  while the inquiry is marked spam or unqualified, since Google cannot retract them.
- New `POST /api/v1/conversion-events` (new `conversions:write` key permission) and
  `GET /api/v1/conversion-events` (`read`). Events match a lead by ID, phone (last 10
  digits) or email, first or latest click. Identical reposts are no-ops; changed values
  become revisions that restate the value under the same transaction ID. Cancel stops
  unsent work; sent conversions are flagged because Google cannot retract them.
- Blocked events are kept with a reason (no lead, no Google click, outside the click
  window, no consent, already counted for this click, …).
- Off by default. Owners turn on automatic sending and choose a consent policy: recorded
  consent per lead, or an attested "does not advertise in the EEA, UK or Switzerland"
  that sends without a consent signal and records who attested and when. Events recorded
  while sending is off are never sent later.
- The delivery worker plans events every tick and refreshes the read-only Google
  destination check when it is older than 24 hours. A failed check holds sends and
  emits `conversion.failed` once. `conversion.*` webhooks for pipeline sends include a
  `pipeline_event` object.
- Pipeline outbox rows (kind `pipeline`) are excluded from the workspace-wide
  invalidation trigger, so booking, inquiry, settings and consent edits do not stall
  them. A destination may now be saved without reviewed qualification/booking actions.
- The Conversions page is now one pipeline-first flow: connect Google and choose the
  account, define and map actions (pick an existing Google action, or **Create in Google
  Ads** for a recoverable VisiConvert action), consent policy, automatic sending with a
  setup checklist, then results. The fixed qualified-lead/booking setup and the manual
  stage/validate/queue workflow moved, unchanged, under **Advanced: manual review**.
- Keys and MCP (`conversions:write`) can create, map and update actions, create a
  Google action for one, and post events; `GET /api/v1/conversion-setup`,
  `/conversion-actions` and read-only Google discovery need `read`. New MCP tools:
  `get_conversion_setup`, `list_google_accounts`, `list_google_conversion_actions`,
  `list_conversion_actions`, `create_conversion_action`, `update_conversion_action`,
  `create_google_conversion_action`, `record_conversion_event`, `list_conversion_events`.
  Keys and agents still cannot connect Google, change the consent policy or turn on
  delivery or automatic sending. Hosted remote MCP needs Rails to offer `conversions:write`.
- Additive migration 0023 (also widens `google_created_actions.kind` for pipeline actions). See [pipeline conversions](docs/PIPELINE_CONVERSIONS.md).

## Unreleased — billable number-purchase keys

- Owners can grant `numbers:purchase` (search, buy, and release SignalWire numbers) in
  API Keys, under a separate billable permission with a cost notice. Its expiration
  defaults to 24 hours.

## Unreleased — DNI number formatting

- Swapped website numbers keep the page's formatting (for example `(602) 555-0105`)
  instead of showing raw E.164. `tel:` links still use E.164.

## Unreleased — signed outbound webhooks

- Send signed events for new interactions, completed calls, inquiry reviews, bookings
  and Google conversion results to up to 10 HTTPS endpoints per workspace, in hosted
  and self-hosted mode. Signing follows Standard Webhooks, with 24-hour overlap on
  secret rotation.
- Record events with database triggers in the same transaction as each change. Nothing
  is recorded until an endpoint exists; demo data never sends.
- At-least-once delivery with about 28 hours of retries, identical bytes on retry, a
  delivery log, redelivery and test events. Endpoints disable after 410 Gone or 5 days
  of failures.
- Hosted workspaces hold deliveries while billing is inactive and resume when it is
  restored. The engine asks Rails per workspace, caches the answer for 5 minutes and
  holds when Rails cannot answer. Held deliveries do not use up retries.
- Reject private, loopback, link-local and reserved destinations, checked at send time
  against the address actually used; no redirects. Self-hosted operators can opt in to
  private destinations with `VISICONVERT_WEBHOOKS_ALLOW_PRIVATE=true`.
- Manage endpoints under `/api/v1/webhooks` with the new `webhooks:write` key permission.
  Additive migration 0022. See [webhooks](docs/WEBHOOKS.md).
- Add a Webhooks screen for workspace owners: create endpoints, copy the secret once,
  send tests, view and redeliver deliveries, pause, rotate and delete.

## Unreleased — reviewed Google conversion delivery

- Add shared Rust delivery for hosted and self-hosted workspaces: validation-only
  requests, explicit opt-in and queuing, bounded retries and receipt polling.
- Preserve original event identities for booking value revisions; stop ambiguous
  sends for review instead of silently replaying them.
- Recheck consent, destination, source revisions and access before sending.
  Cancellations flag already-sent events for manual correction; Google Data Manager
  does not support retractions.
- Add owner controls, delivery history and additive migration 0020. Existing
  preparations remain unsent on upgrade. Microsoft delivery remains future work.

## Unreleased — purchase safety

- Persist purchase phases so rejected budgets and interrupted pre-carrier requests
  can recover without blocking usage or risking a duplicate purchase.
- Preserve and reconcile uncertain reservation holds, including late responses.
- Validate routing conflicts before purchase and again at registration; cap carrier
  attempts per rolling hour, including failures and cleanup.
- Add a bounded operator purchase-reconciliation command and migration 0017.

# Changelog

## Unreleased — number lifecycle safety

- Persist purchase intent and carrier evidence; reconcile ambiguous responses without buying twice, and retry cleanup after registration failures.
- Add workspace-scoped purchase status, full-payload idempotency, and capacity reservations for unresolved requests.
- Claim carrier releases durably, reject restores after deletion starts, and protect website fallback numbers from release.
- Require a separate Rails eligibility check for managed carrier spending; it remains disabled until usage billing is implemented. BYOC purchases remain supported.
- Migration 0015 preserves legacy ambiguous attempts for operator review. See the SignalWire connection guide before a paired rollout.

## Unreleased — remote MCP authorization

- Hosted remote MCP over Streamable HTTP with VisiConvert sign-in, workspace selection, explicit scope approval, rotating tokens, and Connected agents revocation.
- Share MCP schemas and handlers between stdio and HTTP; scope writes and preserve workspace isolation and billing/password-reset checks.
- Requires a paired Rails/engine/gateway rollout; local stdio and API keys remain available.

## Unreleased — agent onboarding and setup API

- Setup screen with manual links, a temporary scoped-key preset, local MCP configuration and a guided prompt; shared by hosted and self-hosted workspaces.
- Public setup evidence, website and DNI endpoints; explicit `setup:write` permission with additive migration 0012. Existing keys retain their scopes.
- MCP site/pool tools preserve version checks and keep provider credentials and carrier routing outside the agent tools. CLI `setup` reports observed activity.
- Per-site and per-number historical evidence distinguishes configuration from received visits, forms and calls; no automatic completion or conversion uploads.
- Make sidebar navigation and workspace locking reachable in short desktop windows.

## Unreleased — SignalWire

- Connect existing SignalWire projects using encrypted API tokens and separate webhook signing keys.
- Verify number ownership, forward signed cXML calls, and share source attribution/DNI with Twilio.
- Select providers in the web app, CLI, MCP and scoped public API; preserve existing Twilio numbers.
- Show account verification separately from signed callback readiness. No carrier routing is changed automatically.


## Unreleased — website source classification

- Shared source classification for hosted and self-hosted website forms and DNI calls: explicit UTMs, Google/Microsoft/Facebook click signals, and bounded referrer matching.
- Preserve raw campaign evidence and highlight conflicts; multiple click providers without an explicit source remain unknown.
- Immutable historical attribution and upgrade-safe session retries; no schema migration or conversion uploads.

## Unreleased — Google connection and destination checks

- Shared owner/operator OAuth flow with PKCE, browser-bound single-use state, encrypted refresh tokens and reconnect/disconnect controls.
- Read-only Google Ads account/action discovery and revision-bound destination checks; no conversion uploads.
- Additive migration 0010, self-hosted OAuth configuration, popup flow, diagnostics and setup guides.

## Unreleased — dependency security update

- Update the PostgreSQL client’s transitive `ctutils` dependency to 0.4.4, fixing RUSTSEC-2026-0385. No application or database schema changes.

## Unreleased — workspace API keys and public API v1

- Shared key management with one-time secrets, hashed storage, scopes, expiration, rotation/revocation and audit history.
- Public v1 inquiry/interaction/booking endpoints with pagination, reports and number registration; OpenAPI contract.
- Hosted keys enforce owner membership, credential version and live Rails access/billing checks; self-hosted keys stay local.
- CLI/MCP support scoped keys and v1 pagination, with secure origin validation for all configuration sources.
- Additive migration 0009; Rails must roll out the internal access check before the engine. Deployed October 23, 2026 to the hosted engine and standalone pilot.

## Unreleased — conversion preparation

- Shared hosted/self-hosted Google Ads destination mapping and conversion preview for qualified inquiries and bookings.
- Explicit advertising consent evidence; tracking consent alone never qualifies an event.
- Durable reviewed staging with stable IDs, exact-cent values, retry deduplication, invalidation, cancellation, and actor history.
- Additive migration 0008, owner/workspace isolation, and database/browser coverage.
- Preparation only: Google OAuth, validation, sending, delivery retries, and retractions remain unimplemented.

## Unreleased — dynamic number insertion

- Shared hosted/self-hosted website pools with atomic session assignments, 30-minute windows and a 30-minute reuse delay.
- Consent-gated, explicitly marked phone/link swapping with safe fallback, expiry, and dynamic page support.
- Owner pool settings and assignment diagnostics; immutable call evidence marks timing matches as inferred and out-of-order/unmatched calls as unknown.
- Additive migration 0007, browser/database regression checks, and self-host installation instructions.


## Unreleased — inquiry grouping

- Explicitly combine repeat calls/forms into one inquiry in both hosted and self-hosted workspaces.
- Interaction timeline, shared qualification/notes, preserved source evidence, and inquiry-level source/revenue reports.
- Separate mistaken associations with bookings following their original interaction; retain review snapshots, reasons, and actor history.
- Workspace isolation and version checks protect concurrent edits, including legacy interaction APIs.
- Migration 0006 preserves existing records without automatically matching people; desktop/mobile and populated-database upgrade coverage.

## Unreleased — website capture

- Shared Sites setup, exact-origin collection, pause/edit controls, field allowlists, and receipt diagnostics.
- Consent-gated standalone tracker with tab-scoped session entry context, recognized campaign/click IDs, and explicit successful-form hooks.
- Atomic retry deduplication, per-site collection limits, workspace isolation, and source evidence in the lead drawer.
- Additive migration 0005 and self-host packaging; no hosted billing dependency or new secret.
- Browser collection remains best effort and browser-reported; no automatic form listeners, number pools, or cross-session attribution yet.

## Unreleased — command line and MCP

- New `visiconvert` binary (`crates/convert-cli`) for status, Twilio connection, numbers, leads, qualification, bookings and the source report, with `--json` output.
- `visiconvert mcp` serves the same operations to AI agents over stdio. It is read-only unless started with `--allow-changes` and never accepts Twilio credentials.
- Built only on existing API endpoints. Twilio webhook URLs are still set by hand.
- Starlight documentation site in `docs-site/` covering setup, concepts, the CLI, MCP and the core API, with `llms.txt`. CLI and MCP reference pages are generated by `visiconvert reference` and a test fails when they are stale.

## Unreleased — shared Twilio connections

- Connect and verify a Twilio account in both self-hosted and hosted workspaces.
- Encrypt provider credentials, verify number ownership, and isolate signed callbacks by workspace.
- Protect account changes with revision checks and preserve routing for legacy environment configurations.
- Include encryption-key setup, backup/restore handling, and connection documentation in self-host packages.
- Provider configuration remains manual; real-call and outage-fallback verification remain launch gates.

## Unreleased

- Optional hosted workspace provisioning and browser-bound single-use login.
- Tenant-scoped short-lived sessions, membership checks, and session revocation.
- Hosted form-lead access while carrier setup remains restricted to the local operator workspace.
- Database/HTTP coverage for tenant isolation, concurrent exchange, expiry, and replay protection.
- Separate Rails account service; no Rails or cloud dependency in the self-hosted package.

## 0.1.0-beta.1 — release candidate

Initial self-hosted beta for a single workspace.

- Rust API, PostgreSQL persistence, production web application, and WASM
  attribution preview in a Docker Compose installation.
- Form leads, qualification, notes, and optimistic edit conflict detection.
- Static campaign-number registration and signed Twilio callback handling.
- Exact-cent USD booking outcomes, revisions, cancellation, and source totals.
- Generated private credentials, repeatable setup and migrations, pre-upgrade
  database backups, and restoration into an empty installation.
- Refusal to overwrite populated databases during restore.

Validation: the initial application image passed all seven disposable self-host
integration checks on 2026-10-21, including restart/reinstall persistence and
restoration with new destination credentials. Twelve installer tests also passed.

This is an early evaluation release. Live carrier/fallback verification,
individual accounts and roles, dynamic number insertion, automatic form capture,
conversion delivery, and hosted billing remain outstanding. No public container
registry image is supplied; installations build from source.
