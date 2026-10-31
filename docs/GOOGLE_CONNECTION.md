# Google connection and destination validation

This shared Rust/Vite feature works in hosted and self-hosted workspaces. It adds
OAuth, read-only Google Ads account/action discovery and validation of saved
conversion destinations, plus explicit creation of workspace-specific import actions. Connecting alone uploads no conversions and changes no account, campaign, action
or billing settings. [Reviewed delivery](CONVERSIONS.md) is a separate opt-in. Live Google configuration and
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

- `https://www.googleapis.com/auth/adwords` for account/action discovery and action creation.
- `https://www.googleapis.com/auth/datamanager` for event validation, delivery and receipt checks.

Google Ads mutations are limited to owner-requested conversion action creation.
Event validation and sending use the separate reviewed Data Manager delivery flow.
Google Cloud production API access and OAuth verification are provider-side setup
requirements. Testing-mode consent apps may restrict users and refresh-token
lifetime. As of October 2026, Google Ads API access is associated with the Cloud
project; this implementation does not send a developer token.

## Workspace workflow

1. Open **Conversions → Sign in with Google** and allow the consent popup.
2. Grant both requested permissions. If your browser isolates the popup, return
   and use **Refresh Google connection**.
3. Account selection opens automatically. Choose an access account if prompted,
   then choose the advertising account by name and ID. For cross-account
   conversion tracking, choose the account that owns the actions.
4. Select qualified leads, bookings, or both and choose **Create conversion
   actions**. Qualified leads count once per click with zero value; bookings count
   each booking with its recorded value. New actions use a 90-day click window
   and secondary status. Choose your bidding outcome in Google Ads after delivery
   verification. Campaign goals and existing action bidding settings are untouched.
5. Alternatively, expand **Use existing actions (advanced)**, map distinct enabled
   import actions, and choose **Save conversion actions**. Creating actions
   replaces saved mappings with the selected outcomes; unselected outcomes are
   unmapped. Existing Google actions are not deleted.
6. **Check saved connection** verifies the saved IDs directly, even if they fall
   outside the first page of discovery results. Changes to settings or credentials
   invalidate the check. A failed check cannot leave an old success current.

Discovery shows up to 200 accounts/actions with a truncation indicator; the
access-account list is capped at 1000. Manual destination IDs remain available
under **Advanced destination settings**.

Creation uses stable workspace-specific names and stores provider IDs separately
from destination mappings. Retries recover partial setup; renamed saved actions
and OAuth reconnects reuse the same IDs. Removed or incompatible actions require
review instead of automatic replacement. If a response is lost, retry with the
same account and outcomes. This operation does not enable delivery or queue events.

“Checks passed” means the account is readable and the selected actions are enabled
import actions owned by that account. It does **not** prove Data Manager upload
permission, acceptance, matching, attribution, or compatibility of a particular
event. Use the separate event validation and delivery controls for those checks.
Existing preparations remain local until explicitly validated and queued.

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
- `POST /api/connections/google/actions/create`: `{revision,settings_version,login,customer,qualified,booking}`; at least one outcome required. See [creation and retry semantics](CONVERSIONS.md#api).
- `POST /api/connections/google/validate`: `{revision,settings_version,login}`.
- `DELETE /api/connections/google`: `{revision}`; removes local credentials.

Migrations `0010_google_connection.sql` and `0021_google_action_setup.sql` are additive; existing Twilio connections,
conversion preparations and API keys are unchanged. Rails needs no new endpoint.

Provider references checked October 31, 2026:
- [Google web-server OAuth](https://developers.google.com/identity/protocols/oauth2/web-server)
- [Google Ads REST account discovery](https://developers.google.com/google-ads/api/rest/common/others)
- [Conversion action fields](https://developers.google.com/google-ads/api/fields/v26/conversion_action)
- [Google Cloud API access](https://developers.google.com/google-ads/api/docs/api-policy/developer-token)
- [Data Manager API access](https://developers.google.com/data-manager/api/devguides/quickstart/set-up-access)

- [Create and configure conversion actions](https://developers.google.com/google-ads/api/docs/conversions/getting-started)
