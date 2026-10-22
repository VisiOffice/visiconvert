# Changelog

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
