# Google connection and destination validation

This shared Rust/Vite feature works in hosted and self-hosted workspaces. It adds
OAuth, read-only Google Ads account/action discovery and validation of saved
conversion destinations. **No conversions are uploaded, and no Google account,
campaign, action or billing setting is changed.** Live Google configuration and
acceptance remain separate from automated verification using a test provider.

## Installation

Configure a Google Cloud project with the Google Ads API and Data Manager API
enabled. Create a **Web application** OAuth client and register the exact callback:

```text
https://YOUR-VISICONVERT-HOST/auth/google/callback
```

The origin comes from `PUBLIC_URL`, or the configured `VISICONVERT_APP_ORIGIN`
for the [unified hosted frontend](HOSTED_FRONTEND.md). Forwarded Host headers
do not control it. Register the exact callback shown in Administrator setup.
HTTPS is required except for exact loopback HTTP origins during local development.
For hosted workspaces using the unified frontend, register
`https://cloud.visiconvert.com/auth/google/callback`. The older
`workspaces.visiconvert.com` callback may remain registered for compatibility.
Separate instances can use separate OAuth clients.

Set these in the engine's private environment. Self-hosters can add them to
`.local/self-host/config.env` (mode 0600); both Compose configurations pass them
through to the application:

```text
VISICONVERT_GOOGLE_CLIENT_ID=YOUR_WEB_CLIENT_ID.apps.googleusercontent.com
VISICONVERT_GOOGLE_CLIENT_SECRET=YOUR_CLIENT_SECRET
```

Keep the existing `VISICONVERT_ENCRYPTION_KEY`; never replace it to enable Google.
Set both OAuth values or neither. Restart/recreate the app after editing them.
These are server settings, never browser fields, public build variables or API
keys. Back up the private configuration along with the database and encryption key.
If restoring under a different OAuth client or origin, reconnect Google. The
installer preserves destination OAuth settings rather than silently importing them.

Google consent configuration needs both scopes:

- `https://www.googleapis.com/auth/adwords` for account/action discovery.
- `https://www.googleapis.com/auth/datamanager` for the forthcoming delivery slice.

Google exposes broad OAuth scopes even though this implementation uses only
read-only Ads methods. The app has no ingestion or mutation client method.
Google Cloud production API access and OAuth verification are provider-side setup
requirements. Testing-mode consent apps may restrict users and refresh-token
lifetime. As of October 2026, Google Ads API access is associated with the Cloud
project; this implementation does not send a developer token.

## Workspace workflow

1. Open **Conversions → Connect Google Ads** and allow the consent popup.
2. Grant both requested permissions. The popup returns to VisiConvert; the main
   workspace retains its in-memory credential. If your browser isolates popups,
   use **Refresh Google connection** after returning.
3. Choose **Browse Google Ads**, then a directly accessible account that provides
   access. Choose an advertising account from its hierarchy or enter its ID.
4. **Load conversion actions**. Enabled `UPLOAD_CLICKS` actions owned by the
   selected account are selectable. Other types, removed actions and actions
   owned elsewhere are shown but disabled. For cross-account tracking, select the
   conversion-owning account as the destination.
5. Map qualification and/or bookings to distinct actions, then choose **Use
   selected actions**. Existing manual destination entry remains available.
6. **Validate saved destination** checks the saved settings against Google again.
   It records the account, selected actions, primary/secondary flags, lookback
   windows, settings revision and check time. Changes to settings or credentials
   invalidate the check. Validation requests clear a previous result before I/O,
   so failed reauthorization never leaves an old success displayed as current.

Discovery shows up to 200 accounts/actions with an explicit truncation indicator.
The access-account list is capped at 1000. This initial validation checks the
first 200 actions; an action outside that list is not marked valid. It does not
silently treat truncated discovery as complete.

“Checks passed” means the account is readable and the selected actions are enabled
import actions owned by that account. It does **not** prove Data Manager upload
permission, acceptance, matching, attribution, or compatibility of a particular
event. Those require the next delivery/validation slice. No synthetic event is
sent to Google by this feature. Existing preparations remain local.

## Credentials and authorization

Only hosted workspace owners or standalone operators can manage the connection.
Scoped public API keys, CLI/MCP sessions and Rails service keys cannot manage it.
OAuth state expires after ten minutes and is single-use, browser-cookie-bound,
PKCE-protected and tied to the initiating owner session or operator credential.
The callback rechecks owner access and connection revision. A new attempt or
disconnect invalidates an older attempt, including an already-consuming callback.

Refresh tokens and PKCE verifiers use authenticated encryption with independent
Google purpose/workspace contexts. Tokens, codes, upstream error bodies and
ciphertext are never returned through the application API. Access tokens live only
for a request. Provider URLs are fixed, redirects disabled and response sizes and
timeouts bounded. Only test builds can substitute a local provider origin.

**Disconnect Google** removes local credentials and pending attempts, increments
the connection revision and clears validation. It does not revoke the Google-wide
grant, which could also serve another workspace. Users can separately remove the
grant in their Google Account's third-party access settings. Connect, disconnect
and destination checks appear in conversion history without secrets.

## Private API

- `GET /api/connections/google`: configuration, connection revision and check status.
- `POST /api/connections/google/start`: `{revision}`; returns an authorization URL
  and sets the HttpOnly/SameSite browser cookie.
- `GET /auth/google/callback`: provider callback; requires matching state/cookie.
- `GET /api/connections/google/accounts`: directly accessible login account IDs.
- `GET /api/connections/google/accounts?login=ID`: bounded account hierarchy.
- `GET /api/connections/google/actions?login=ID&customer=ID`: action metadata.
- `POST /api/connections/google/validate`: `{revision,settings_version,login}`.
- `DELETE /api/connections/google`: `{revision}`; removes local credentials.

Migration `0010_google_connection.sql` is additive; existing Twilio connections,
conversion preparations and API keys are unchanged. Rails needs no new endpoint.

Provider references checked October 23, 2026:
- [Google web-server OAuth](https://developers.google.com/identity/protocols/oauth2/web-server)
- [Google Ads REST account discovery](https://developers.google.com/google-ads/api/rest/common/others)
- [Conversion action fields](https://developers.google.com/google-ads/api/fields/v26/conversion_action)
- [Google Cloud API access](https://developers.google.com/google-ads/api/docs/api-policy/developer-token)
- [Data Manager API access](https://developers.google.com/data-manager/api/devguides/quickstart/set-up-access)
