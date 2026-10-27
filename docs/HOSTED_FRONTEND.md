# One frontend for hosted VisiConvert

The Vite/React app renders both account management and the product. Rails owns
accounts, verified email, sessions, workspace provisioning and Stripe billing.
Rust owns workspace data, integrations and API keys. Astro remains the marketing
site. This implementation is opt-in. It is deployed on the hosted service as of
October 23, 2026, with registration open and billing in Stripe test mode.

## Routing

The hosted browser uses one origin, `https://cloud.visiconvert.com`:

| Path | Service |
| --- | --- |
| Account pages, `/app/<workspace-id>`, assets, `/runtime.json` | Rust serves the Vite build |
| `/api/cloud/*` | Rails JSON account API |
| `/api/*`, `/api/v1/*`, `/auth/*` | Rust |
| `/webhooks/stripe`, `/internal/api-access`, `/up` | Rails |

The gateway blocks other `/internal/*` routes. Rails uses the separately
configured engine origin for authenticated provisioning and revocation calls.
The customer gateway does not grant access to these endpoints.

Set `VISICONVERT_APP_ORIGIN` on the hosted Rust engine to the exact browser
origin. It must equal `VISICONVERT_CLOUD_ORIGIN`. Keep `PUBLIC_URL` at the existing
engine origin (`https://workspaces.visiconvert.com`) so carrier callback URLs,
signature validation and existing integration clients retain their origin.
`GET /runtime.json` exposes only the optional app origin, never credentials.
The frontend enters account mode only when this origin matches its current URL.

Self-hosted installations leave `VISICONVERT_APP_ORIGIN` and the cloud settings
unset. The same build retains the existing operator-key login and needs no Rails
service, subscription, or external account. The original hosted handoff remains
available during migration.

## Sessions and account actions

Rails retains its signed HttpOnly cookies and CSRF protection. `GET
/api/cloud/session` supplies the current user and a CSRF token; browser mutations
send `X-CSRF-Token`. Membership and paid-access checks stay on the server. Login
rotates the Rails session; signup still respects `CLOUD_SIGNUPS_ENABLED`.

Opening a workspace uses three same-origin requests: Rust `/auth/prepare`
sets a short-lived HttpOnly cookie; Rails `/api/cloud/workspaces/<id>/launch`
checks membership and billing and returns a single-use code; Rust
`/auth/exchange` binds and consumes it. The 15-minute bearer token stays in memory.
Reloading `/app/<id>` repeats this flow using the existing Rails session.
Expiry returns to the workspace list; failed writes are never replayed.

Sign out revokes the currently held Rust token and the Rails session. It does not
revoke sessions in other tabs/devices; they keep the existing bounded lifetime.
Password reset retains the existing durable, cross-workspace revocation flow.
Email/reset link fragments are removed from history before asynchronous work.
Stripe Checkout and its customer portal remain external Stripe pages; URLs are
restricted to their expected HTTPS hosts.

## Google callback migration

When this mode is enabled, Google uses
`https://cloud.visiconvert.com/auth/google/callback`. Register that exact redirect
URI on the hosted OAuth client **before** enabling the setting. Retain the old
`https://workspaces.visiconvert.com/auth/google/callback` during migration and
rollback. Client secrets remain server-side. Self-hosted Google callbacks still
use that installation's own `PUBLIC_URL`.

## Development and verification

For Vite development on `http://127.0.0.1:5173`, run Rails on port 3200 and Rust
on 3100. Set both Rust cloud/app origins and Rails `CLOUD_PUBLIC_ORIGIN` to 5173;
keep Rust `PUBLIC_URL` and Rails `VISICONVERT_ENGINE_PUBLIC_ORIGIN` at 3100.
The Vite proxy routes account APIs to Rails and product APIs to Rust.
Use separate databases and matching server-side service keys as documented in
[the hosted protocol](HOSTED_PROTOCOL.md).

The private Cloud repository contains the gateway template, deployment overlay,
Rails JSON/security tests and `script/verify-unified.py`. Its browser suite uses
real disposable Rails, worker, Rust, PostgreSQL, NGINX and Chromium services,
capturing mail locally. No real Stripe payment, Google authorization or email
delivery is part of that verification. The existing engine browser suite must
also pass to verify self-hosted behavior.
