# Optional hosted account integration

The Rust engine remains independently self-hostable. Leave both cloud settings unset for the existing workspace-key mode. The separate VisiConvert Cloud service is Rails 8.1 with its own PostgreSQL database; it does not implement lead or attribution logic.

Enable the optional protocol with:

- `VISICONVERT_CLOUD_SERVICE_KEY`: independent random 64-character hex server secret, different from `VISICONVERT_API_KEY`.
- `VISICONVERT_CLOUD_ORIGIN`: fixed Rails origin, such as `https://cloud.visiconvert.com`.
- `PUBLIC_URL`: engine origin, such as `https://app.visiconvert.com`.

Origins must be HTTPS, except loopback HTTP for development. Apply all migrations before starting the app. Never place the service key in the browser. Existing installations need neither cloud setting nor a Cloud account.

## Provisioning

`POST /internal/cloud/workspaces`, authenticated with `Authorization: Bearer <service key>`, accepts `{workspace_id, user_id, name}`. IDs are UUIDs. Repeating the identical request is safe; a conflicting owner/name or existing local workspace is rejected. Success returns `{workspace_id, status: "ready"}`. This creates a logical tenant and owner membership, not customer infrastructure.

The service credential cannot access the private lead API. Operator credentials cannot call cloud provisioning. The operator key continues to select only its configured local workspace.

## Browser handoff

1. The browser visits `/auth/start?workspace_id=<uuid>`. Rust creates a 5-minute HttpOnly SameSite=Lax cookie restricted to `/auth`, then redirects to the configured Cloud origin's `/workspaces/<uuid>/authorize?state=<SHA256 cookie secret>`.
2. Cloud authenticates the user, checks membership, and asks them to continue using a CSRF-protected POST. Its server calls `POST /internal/cloud/launch` with `{workspace_id, user_id, state, credential_version}` and the service credential. The nonnegative integer version must match the engine identity. Omission means version zero for rollout compatibility; it never bypasses a later revocation.
3. Rust requires enabled workspace membership and returns `{code, expires_in: 60}`. Cloud redirects to the fixed engine origin with `/#launch=<code>`; it must not log that Location header.
4. The app removes the fragment before other asynchronous work, then sends `POST /auth/exchange` with `{code}`, the browser cookie, and the engine Origin. Rust atomically consumes a matching, unexpired code and returns `{token, expires_in: 900}`. Another browser or a repeated exchange cannot use it.
5. The token stays in browser memory and authenticates `/api/*`. Every request resolves the tenant from the stored session and checks enabled membership and expiry. `DELETE /api/session` revokes that session.

`hosted_users`, `hosted_workspaces`, `hosted_memberships`, `hosted_launch_codes`, and `hosted_sessions` are engine-owned records. Only hashes of codes and bearer tokens are stored. Cloud's user UUID is an external identity reference, not access on its own. Disabled memberships/workspaces or a stale credential version stop subsequent session access. In-flight requests may finish normally.

## Credential revocation

`POST /internal/cloud/revoke-user`, authenticated with the service key, accepts `{user_id, credential_version}` with a positive integer version. It atomically advances the user's version (never decreasing it) and deletes every older launch code and session across that user's workspaces. Success returns `{user_id, credential_version}` with the engine's current version. Repeating the same version preserves credentials legitimately issued at that version. Provisioning an identity later does not undo a preexisting revocation.

Issuance, exchange, and revocation lock the same identity row, preventing concurrent exchanges or delayed launch requests from restoring old access. The caller must persist its version before issuing the revocation and must not launch using the new version until recovery completes. Cloud persists the proposed password's bcrypt digest and a revocation job together, blocks sign-in while pending, then activates the password after the engine acknowledges. A timeout keeps the reset pending and retryable; it is not reported as a successful reset. Existing engine access cannot be revoked while the engine is unreachable and retains its expiry until revocation is delivered.

Deploy the engine migration and implementation before the updated Cloud workers/web. Keep paired database backups; do not downgrade to an engine that ignores credential versions after using this protocol.

## Current limits

### Durable integration credentials (deployed October 23, 2026)

Workspace owners can issue scoped API keys for `/api/v1` from the engine UI.
These keys are separate from the 15-minute browser sessions below. Keys retain
the issuing owner and credential version; engine membership disablement and
password resets invalidate them. Creation and every hosted API-key request also
call Rails' service-authenticated `POST /internal/api-access`, sending only the
workspace ID, owner UUID and credential version. Rails verifies current ownership,
membership, verified email, completed credential reset, workspace readiness and
paid access. A denied entitlement returns 403; an unavailable check fails closed
with 503. Self-hosted keys do not call Rails.

For this slice, deploy the new Rails endpoint **before** engine migration 0009
and the updated engine/CLI. This order differs from the earlier credential-version
protocol rollout above. No service secret or operator key is handed to an
integration. See [public API and key lifecycle](PUBLIC_API.md) for scopes,
pagination, rate limits, revocation and backup-restoration precautions.

Hosted workspaces support form leads, qualification, bookings, and the same
[Twilio connection flow](TWILIO_CONNECTIONS.md) as self-hosted workspaces.
Phone-number registration requires verified workspace credentials and checks
number ownership. Credentials are encrypted by the engine; scoped callback URLs
resolve the workspace and validate its provider signature independently of
Rails and billing. The engine operator must configure its encryption key.
Legacy environment callbacks remain scoped to the default operator workspace.

Sessions expire after 15 minutes without refresh; reopen through Cloud after a reload or expiry. Completed password resets revoke engine access. Ordinary Rails logout and subscription cancellation do not yet revoke existing engine sessions; engine logout revokes its own session. Session refresh, invitations, email verification, and tenant deletion remain future work. Cloud owns billing and applies its entitlement checks before issuing launch requests.

## Verification

The Docker `verify` target includes isolated PostgreSQL/HTTP tests of service-key boundaries, idempotent provisioning, membership, concurrent exchange, code/session expiry, logout, cross-tenant lead isolation, and revocation races. It also runs the existing self-hosted database and browser workflow. Cloud maintains its own Rails, onboarding, and password-recovery browser suites.
