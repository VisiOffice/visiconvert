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
2. Cloud authenticates the user, checks membership, and asks them to continue using a CSRF-protected POST. Its server calls `POST /internal/cloud/launch` with `{workspace_id, user_id, state}` and the service credential.
3. Rust requires enabled workspace membership and returns `{code, expires_in: 60}`. Cloud redirects to the fixed engine origin with `/#launch=<code>`; it must not log that Location header.
4. The app removes the fragment before other asynchronous work, then sends `POST /auth/exchange` with `{code}`, the browser cookie, and the engine Origin. Rust atomically consumes a matching, unexpired code and returns `{token, expires_in: 900}`. Another browser or a repeated exchange cannot use it.
5. The token stays in browser memory and authenticates `/api/*`. Every request resolves the tenant from the stored session and checks enabled membership and expiry. `DELETE /api/session` revokes that session.

`hosted_workspaces`, `hosted_memberships`, `hosted_launch_codes`, and `hosted_sessions` are engine-owned records. Only hashes of codes and bearer tokens are stored. Cloud's user UUID is an external identity reference, not access on its own. Disabled memberships/workspaces stop subsequent session access. In-flight requests may finish normally.

## Current limits

Hosted workspaces support form leads, qualification, and bookings. Their bootstrap response identifies `hosted: true`, hides operator callback URLs, and disables phone-number writes. Existing Twilio callbacks remain scoped to the configured self-hosted workspace. Per-tenant carrier credentials and callback resolution must precede hosted calling.

Sessions expire after 15 minutes without refresh; reopen through Cloud after a reload or expiry. Rails logout/password reset currently invalidates Rails sessions only, while an existing engine session remains valid until its expiry or explicit engine logout. Unified revocation, refresh, invitations, email verification, billing, quotas, and tenant deletion are future work.

## Verification

The Docker `verify` target includes `cloud_handoff_enforces_membership_browser_binding_expiry_and_single_use`, an isolated PostgreSQL/HTTP test of service-key boundaries, idempotent provisioning, membership, concurrent exchange, code/session expiry, logout, and cross-tenant lead isolation. It also runs the existing self-hosted database and browser workflow. Cloud maintains its own Rails and full onboarding browser suites.
