# Hosted remote MCP with VisiConvert OAuth

The hosted endpoint is `https://cloud.visiconvert.com/mcp`. This feature requires
**both** the new engine and Rails release, Rails migration `20261027200000`, and
the updated unified gateway. Publishing docs or deploying only the frontend does
not enable it. The local stdio MCP and workspace API keys remain supported for
hosted and self-hosted installations. Standalone engines return 404 at `/mcp`.

## Customer flow

Add the URL as a custom connector. In Claude, choose **Register automatically**
for the OAuth client if prompted. This release supports Dynamic Client
Registration (DCR), not Client ID Metadata Documents or confidential clients.
Sign in with your **VisiConvert** email and password, select a ready workspace,
and approve permissions. Only verified owners with workspace access can approve.
Read is required; write checkboxes start unchecked. The client must request the
write scopes it needs (`setup:write`, `numbers:write`, `leads:write`,
`bookings:write`). Only requested scopes can be approved. If the client requests
only read, reconnect with the additional scopes before attempting setup changes.

The agent receives an OAuth token, never the account password or a workspace API
key. Manage/revoke connections at **Account → Connected agents**. Disconnecting
in the client alone may not revoke its authorization; revoke it in VisiConvert.
Each connection is restricted to one workspace and expires after 90 days.

## Protocol and boundaries

- Rust serves stateless Streamable HTTP POST `/mcp`, JSON responses, and 202 for
  notifications. Authenticated GET/DELETE return 405; no persistent SSE stream,
  session IDs, batch requests, or legacy HTTP+SSE transport.
- Supported HTTP protocol versions: 2025-11-25, 2025-06-18, 2025-03-26.
- Rails serves `/.well-known/oauth-authorization-server`,
  `/.well-known/oauth-protected-resource/mcp` (also its root alias),
  `/oauth/register`, `/oauth/token`, and `/oauth/revoke`.
- Vite serves `/mcp/authorize`; session/consent APIs remain cookie and CSRF
  protected. The signed consent request binds client, exact registered callback,
  PKCE challenge, state, requested scopes, user, browser session, and a unique
  nonce, with a ten-minute lifetime. Approval is single use.
- Public clients use authorization code with mandatory S256 PKCE and an explicit
  `resource` equal to the canonical `/mcp` URL at authorization and token exchange.
  Redirects match exactly. DCR accepts HTTPS callbacks only, with no URL fetches.
- Codes last one minute; access tokens last fifteen minutes; rotating refresh
  tokens last thirty days, bounded by the ninety-day connection lifetime.
  Credentials are stored as SHA-256 digests. Reusing a consumed code (with its
  valid verifier) or refresh token revokes the connection's entire token family.
- Every HTTP MCP request introspects its token through the service-authenticated
  `/internal/mcp-access`. Every tool API request rechecks the specific grant,
  billing, ownership, verified email, reset status and credential version in
  Rails, plus enabled membership/version in the engine. Failures deny access.
  Already-authorized in-flight writes may finish after revocation.
- Rust uses the same tool schemas/handlers as stdio. It generates a sixty-second
  HMAC delegation for loopback `/api/v1` calls only. This credential never enters
  the browser/client. Unknown scopes, expired/modified assertions, standalone
  use, and unapproved tools are rejected. Audit actors identify the MCP connection.
- MCP tokens cannot authenticate ordinary `/api/v1` requests. Browser sessions
  and API keys cannot authenticate `/mcp`. Provider credentials and billing are
  absent from the tool surface. Google Ads delivery remains disabled.

## Operator configuration

Rails: `VISICONVERT_CLOUD_ORIGIN` is the exact canonical origin (defaults to
`https://cloud.visiconvert.com`). Set it explicitly for other installations;
it must equal the engine's existing `VISICONVERT_CLOUD_ORIGIN`. Never derive the
issuer/resource from request Host headers. Engine:
`VISICONVERT_MCP_LOOPBACK_ORIGIN` defaults to `http://127.0.0.1:3100`; change it if
the engine listens on another port. Only literal loopback HTTP origins are allowed.
`VISICONVERT_MCP_ALLOWED_ORIGINS` is a comma-separated exact-origin allowlist for
browser MCP clients, defaulting to `https://claude.ai,https://claude.com`; the
canonical cloud origin is always allowed. Server-to-server clients may omit Origin.

Use the updated `cloud/deploy/unified-nginx.conf.template`: OAuth metadata and
machine endpoints go to Rails, `/mcp` goes to Rust, and `/mcp/authorize` falls
through to Vite. Both internal access endpoints require the existing independent
cloud service key. Configure no caching or challenge pages on the MCP/OAuth
machine endpoints. Keep callback credentials and Authorization headers out of logs.

DCR and token endpoints have rate limits; MCP is limited per connection. Run
`bin/rails mcp:prune` periodically to remove expired access credentials, grants
expired/revoked for over seven days, and unreferenced client registrations older
than thirty days. Consumed codes/refresh tokens stay until their grant is removed,
so replay detection survives rotation. Back up Rails and engine databases together.

## Validation and rollout

Run Rails tests, Rust tests/Clippy, and the production Vite build. The disposable
`cloud/script/verify-unified.py --engine <root> --engine-image <matching-verify-image>`
includes a real sign-in/consent/refresh/revocation browser flow and desktop/mobile
screenshots. Build the matching verify image first; an old cached image cannot
verify new engine routes. `cloud/test/verification/remote_mcp_http.rb` also tests
real Rails/Rust/Postgres HTTP flows on isolated loopback test services; invoke it
with `bin/rails runner` only in a `_test` database. It revokes fixture credentials.

Roll out the additive Rails migration, paired Rails/engine images, and gateway;
verify discovery, login/consent, a read, an explicitly authorized write, revocation,
backups and monitoring. Preserve both old images for rollback; do not roll back
schema while credentials might exist. Final acceptance is connecting an actual
Claude custom connector to the deployed URL. Local verification does not prove
that a client's account plan or Cloudflare policy permits the live connection.

Protocol references: [MCP authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization),
[Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
