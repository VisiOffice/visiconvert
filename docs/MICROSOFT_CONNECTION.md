# Microsoft Advertising connection

Microsoft Advertising (Bing) is a second destination for [pipeline
conversions](PIPELINE_CONVERSIONS.md). The same actions and events that go to Google
can also be uploaded to Microsoft as offline conversions, matched by the Microsoft
click ID (`msclkid`) that the website tracker captures. It works the same in hosted and
self-hosted workspaces and needs engine migration 0027.

Connecting uploads nothing. Events are sent only when all of these hold: Microsoft is
connected, an ad account is saved, the action is mapped to a Microsoft goal, and the
shared delivery switch and automatic sending are both on.

> **Status.** The Microsoft calls follow Microsoft's published Bing Ads API v13
> reference and are tested against a local mock provider only. They have not yet been
> run against live Microsoft Advertising; treat the first live connection as the
> acceptance test.

## Installation (administrator)

1. Register an application in Microsoft Entra ID (app registrations) with
   **Accounts in any organizational directory and personal Microsoft accounts**
   (multitenant), so customers can sign in with their own Microsoft login.
2. Add a **Web** redirect URI with the exact callback:

   ```text
   https://YOUR-VISICONVERT-HOST/auth/microsoft/callback
   ```

   The origin comes from `PUBLIC_URL`, or `VISICONVERT_APP_ORIGIN` for the
   [unified hosted frontend](HOSTED_FRONTEND.md). Hosted workspaces use
   `https://cloud.visiconvert.com/auth/microsoft/callback`. **Conversions →
   Administrator setup** shows the exact value for an installation.
3. Create a client secret for the application.
4. Get a Microsoft Advertising **developer token** for the VisiConvert operator (one
   token for the whole installation, from the Microsoft Advertising developer portal).
   Customers never see or provide it.
5. Set all three values in the engine's private environment, or none:

   ```text
   VISICONVERT_MICROSOFT_CLIENT_ID=APPLICATION_CLIENT_ID_GUID
   VISICONVERT_MICROSOFT_CLIENT_SECRET=YOUR_CLIENT_SECRET
   VISICONVERT_MICROSOFT_DEVELOPER_TOKEN=YOUR_DEVELOPER_TOKEN
   ```

   Self-hosters can add them to `.local/self-host/config.env` (mode 0600); `compose.yaml`
   passes them through. They require the existing `VISICONVERT_ENCRYPTION_KEY`; never
   replace that key to enable Microsoft. These are server settings, never browser
   fields, public build variables or API keys. Restart the app after changing them.

Without these values the Microsoft section in Conversions, `get_conversion_setup` and
`GET /api/v1/conversion-setup` report Microsoft as unavailable and nothing else changes.
Changing the client ID or the redirect origin requires each workspace to reconnect.

### What VisiConvert asks Microsoft for

- OAuth 2.0 authorization code flow with PKCE (S256) on the `common` v2.0 endpoint,
  scope `https://ads.microsoft.com/msads.manage offline_access`, `prompt=select_account`.
- Microsoft rotates refresh tokens. VisiConvert stores the latest one, encrypted with
  the workspace's encryption context, and replaces it on every refresh.
- SOAP v13 operations: `GetUser` and `SearchAccounts` (Customer Management) to list ad
  accounts; `GetConversionGoalsByIds`, `AddConversionGoals`, `ApplyOfflineConversions`
  and `ApplyOfflineConversionAdjustments` (Campaign Management).

## Workspace workflow

1. **Conversions → Send to Microsoft Advertising → Connect Microsoft Advertising.**
   Sign in with a Microsoft user who can manage the ad account and accept the
   permission. Only the workspace owner (or self-hosted operator) can connect; API keys
   and agents cannot.
2. Choose the **Microsoft Advertising account** and save it. Only active accounts can
   be chosen. VisiConvert derives the customer (manager) ID to operate from.
3. For each action, choose an existing **offline conversion goal**, or **Create goal in
   Microsoft**. Uploads refer to the goal by its exact name.
4. **Check Microsoft destination** confirms the account is active and every mapped goal
   is an active offline conversion goal. VisiConvert repeats the check automatically
   after that (at most every 10 minutes while something is waiting).
5. **Review other goals** lists other goals in the account that could count the same
   leads: another active offline goal (perhaps fed by an older uploader) or a website
   goal in the same category that counts for bidding.
6. Turn on delivery and automatic sending (shared with Google) if they are not on yet.

A created goal is named `VisiConvert - {action key} ({workspace id})`, counts `Unique`
for once-per-click actions and `All` for every-event actions, uses a 90-day window and
variable revenue. Creating one also:

- makes Microsoft wait about **two hours** before it applies uploads for that goal.
  Eligible events wait, and the checklist shows when the goal is ready;
- turns on Microsoft click ID auto-tagging for the account (Microsoft does this for
  every new goal);
- counts the goal for bidding. Change that in Microsoft Advertising if it should only
  report.

If a creation is interrupted, a retry finds the goal by the recorded ID or by name
instead of creating a second one.

## What is sent

| Event | Microsoft request |
|---|---|
| First eligible revision | `ApplyOfflineConversions`: goal name, `msclkid`, conversion time, value and `USD` |
| New revision with a different value | `ApplyOfflineConversionAdjustments` **Restate** with the new value |
| Event canceled | `ApplyOfflineConversionAdjustments` **Retract** |

Adjustments always use the original upload's account, goal, click and conversion time.
A revision that changes the click or the conversion time is not sent (it is shown with
the reason). Unlike Google, a canceled event that already reached Microsoft is retracted
automatically.

Eligibility, checked per event:

- the lead has a Microsoft click (`msclkid`). If a lead has a Google click too, the
  Google click is preferred for lead matching, and the Microsoft click is used for the
  Microsoft upload;
- the event happened after the ad visit, within the goal's conversion window (90 days
  for created goals), and within the last 90 days;
- the consent policy allows it (shared with Google: `per_lead` needs recorded consent,
  `not_required_outside_eea` needs the owner's attestation);
- inferred calls (matched to a website visit by DNI timing) need `allow_inferred_calls`,
  the same opt-in as Google;
- the event was recorded after the action was mapped to its Microsoft goal. Events from
  before the mapping are never sent, so switching off an older uploader does not double
  count.

## Delivery guarantees

- Each request is armed (`attempted_at`) before it leaves. If the response is lost or
  ambiguous the delivery becomes **uncertain** and is never sent again automatically.
- Microsoft per-item errors (`PartialErrors`) and faults are classified:
  - **retry** (with backoff from 30 seconds to an hour): throttling, internal errors
    and connection failures before a response;
  - **goal not ready** (the new goal's two-hour wait): retried every 30 minutes, up to
    10 times;
  - **needs review**: authorization errors (expired consent, no access to the account).
    These are requeued after the owner reconnects;
  - **failed**: any other explicit rejection, with the Microsoft error code.
- Microsoft ignores duplicates with the same click and conversion time ("the first
  instance will be used"), so a retry after an explicit error cannot double count.
- Delivery outcomes raise the same `conversion.delivered` and `conversion.failed`
  webhooks as Google, with provider `microsoft`.

## Agents and API keys

| Tool (MCP) | REST (`/api/v1`) | Scope |
|---|---|---|
| `list_microsoft_accounts` | `GET /connections/microsoft/accounts` | `read` |
| `list_microsoft_goals` | `GET /connections/microsoft/goals[?account_id=]` | `read` |
| `set_microsoft_destination` | `PUT /conversion-setup/microsoft-destination` `{revision, destination_version, account_id}` | `conversions:send` |
| `set_microsoft_action_goal` | `PUT /conversion-actions/{key}/microsoft-goal` `{version, goal_name}` | `conversions:write` |
| `create_microsoft_goal` | `POST /conversion-actions/{key}/microsoft-goal` `{version, category?}` | `conversions:write` |
| `check_microsoft_destination` | `POST /conversion-setup/microsoft-check` `{revision, destination_version}` | `conversions:write` |
| `review_microsoft_conversion_setup` | `GET /conversion-setup/microsoft-review` | `read` |

`revision` and `destination_version` come from the `microsoft` section of
`GET /api/v1/conversion-setup` (`get_conversion_setup`), which also lists the Microsoft
next steps. Google's `ready`, `summary` and `next_steps` are unchanged. Each event in
`list_conversion_events` adds `delivery.microsoft` with its Microsoft status
(`pending`, `blocked`, `queued`, `delivered`, `retracted`, `uncertain`, `failed` or
`canceled`), reason and Microsoft tracking ID. No key or agent can connect Microsoft.

## Limits and what is not verified

- Not yet run against live Microsoft Advertising (see Status above).
- Agency setups where the ad account is reached through a linked manager account are
  handled by deriving the customer ID from `SearchAccounts`; this is unverified live.
- `allow_inferred_calls` is stored with the Google destination settings, so a workspace
  that only uses Microsoft cannot opt in to inferred calls yet.
- USD only. Enhanced conversions (hashed email/phone) are not sent.
- The goal review ignores in-store transaction goals.
- Disconnecting keeps the saved account and goal mappings; nothing is sent until the
  owner reconnects.

## Microsoft references

- [Import offline conversions](https://learn.microsoft.com/en-us/advertising/guides/import-offline-conversions?view=bingads-13)
- [ApplyOfflineConversions](https://learn.microsoft.com/en-us/advertising/campaign-management-service/applyofflineconversions?view=bingads-13),
  [OfflineConversion](https://learn.microsoft.com/en-us/advertising/campaign-management-service/offlineconversion?view=bingads-13)
- [ApplyOfflineConversionAdjustments](https://learn.microsoft.com/en-us/advertising/campaign-management-service/applyofflineconversionadjustments?view=bingads-13),
  [OfflineConversionAdjustment](https://learn.microsoft.com/en-us/advertising/campaign-management-service/offlineconversionadjustment?view=bingads-13)
- [AddConversionGoals](https://learn.microsoft.com/en-us/advertising/campaign-management-service/addconversiongoals?view=bingads-13),
  [GetConversionGoalsByIds](https://learn.microsoft.com/en-us/advertising/campaign-management-service/getconversiongoalsbyids?view=bingads-13),
  [OfflineConversionGoal](https://learn.microsoft.com/en-us/advertising/campaign-management-service/offlineconversiongoal?view=bingads-13)
- [GetUser](https://learn.microsoft.com/en-us/advertising/customer-management-service/getuser?view=bingads-13),
  [SearchAccounts](https://learn.microsoft.com/en-us/advertising/customer-management-service/searchaccounts?view=bingads-13)
- [OAuth consent](https://learn.microsoft.com/en-us/advertising/guides/authentication-oauth-consent?view=bingads-13),
  [tokens](https://learn.microsoft.com/en-us/advertising/guides/authentication-oauth-get-tokens?view=bingads-13),
  [refresh](https://learn.microsoft.com/en-us/advertising/guides/authentication-oauth-refresh?view=bingads-13)
- [Operation error codes](https://learn.microsoft.com/en-us/advertising/guides/operation-error-codes?view=bingads-13)
