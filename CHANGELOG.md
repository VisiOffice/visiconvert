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
