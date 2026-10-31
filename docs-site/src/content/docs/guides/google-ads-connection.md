---
title: Connect Google Ads
description: Connect Google, choose accounts and import actions, and check your conversion destination.
---

VisiConvert can connect to Google and check conversion destinations in both hosted
and self-hosted workspaces. Connecting alone uploads nothing. Use [reviewed delivery](/guides/google-ads-preparation/) to validate and explicitly queue events.

## Connect

Open **Conversions → Sign in with Google**. Allow the popup and grant Google Ads
and Data Manager permissions. Your workspace stays open while Google asks for
consent. If your browser isolates the popup, return to VisiConvert and choose
**Refresh Google connection**.

Only the workspace owner or self-hosted operator can connect Google. Scoped API
keys, the CLI and MCP do not manage provider credentials.

## Choose and check actions

1. After Google sign-in, account selection opens automatically. If there is more
   than one access account, choose the Google Ads account or manager account that
   provides access to your business.
2. Choose the **Advertising account** by name and ID. Conversion actions load
   automatically. Google may use your legal business name rather than your site
   or VisiConvert workspace name. If the account is missing, expand **Can’t find
   your account? Enter its ID**.
3. Choose **Qualified leads**, **Bookings**, or both, then **Create conversion
   actions**. This creates enabled Google Ads import actions and saves their IDs.
   Qualified leads count once per click with zero value; bookings count each
   booking using its recorded value. Both use a 90-day click window.
4. Choose **Check saved connection**. Delivery remains unchanged; setup never
   queues conversions. New actions are **secondary (reporting only)**. After
   verifying delivery, choose your bidding outcome and review campaign goals in
   Google Ads. Setup does not change existing campaign goals or bidding settings.

Names include the workspace ID so workspaces using the same Ads account stay
separate. Repeating setup reuses actions instead of creating duplicates, including
when Google's response was lost. Saved IDs survive renaming and disconnecting.
If an action was removed or its configuration changed, setup stops for review
instead of replacing it. Retry an interrupted setup with the same account and
outcomes to finish saving any actions already created.

To reuse legacy actions, expand **Use existing actions (advanced)**, select enabled
import actions owned by the account, and choose **Save conversion actions**.
Do not send the same outcome through both your old uploader and VisiConvert.
Creating actions replaces the saved mappings with the selected VisiConvert
outcomes; unselected outcomes become unmapped. Existing Google actions are never
deleted. Changing mappings invalidates previous destination and delivery checks.

Reopening setup restores saved mappings when the account is found. Use
**Refresh Google Ads accounts** to retry discovery or pick up access changes.
Manual destination IDs are available under **Advanced destination settings**.

For cross-account tracking, use the account that owns the conversion actions.
The UI shows the owning account when Google returns it. Discovery lists are
limited to 200 entries and show when results are truncated.

A successful check confirms readable account access and compatible enabled import
actions at the displayed time. It does not prove upload permission, event
acceptance or ad attribution. Changing the destination or reconnecting Google
invalidates the check. See [reviewed delivery](/guides/google-ads-preparation/) for event validation,
receipts and supported value corrections.

## Self-hosted setup

Create a Google Cloud project, enable the Google Ads API and Data Manager API,
and create a **Web application** OAuth client. Configure the consent screen for
the `adwords` and `datamanager` scopes and add your users while the app is in
testing mode. Google controls OAuth verification and production API access.

Register this exact authorized redirect URI, using your VisiConvert origin:

```text
https://YOUR-VISICONVERT-HOST/auth/google/callback
```

Add these values to your private `.local/self-host/config.env`, retaining its
0600 permissions, then recreate the app through the self-host installer:

```text
VISICONVERT_GOOGLE_CLIENT_ID=YOUR_WEB_CLIENT_ID.apps.googleusercontent.com
VISICONVERT_GOOGLE_CLIENT_SECRET=YOUR_CLIENT_SECRET
```

Keep the existing encryption key and include the configuration file in backups.
Never put these secrets in a website script, public repository or frontend build.
Hosted workspaces use the operator's configured OAuth application.

## Troubleshooting

- **Not configured:** the administrator must set both client variables and retain
  the server encryption key.
- **Popup blocked:** allow popups for your VisiConvert origin.
- **Could not connect:** retry, grant both permissions, and check the exact
  callback URI. OAuth attempts expire after ten minutes; a new attempt replaces
  the previous one.
- **Access denied:** check the Google user's account access, enabled APIs and
  the Cloud project's approved Google Ads API access level.
- **Authorization expired:** reconnect. Testing-mode Google apps can have limited
  refresh-token lifetimes.
- **Destination needs attention:** check action ownership, type and status. A
  saved action ID is not automatically a valid import action.

**Disconnect Google** deletes credentials from this workspace. It leaves other
workspaces alone and does not revoke the entire Google grant. You can separately
remove VisiConvert in your Google Account's third-party access settings.
