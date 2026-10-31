---
title: Connect Google Ads
description: Connect Google, choose accounts and import actions, and check your conversion destination.
---

VisiConvert can connect to Google and check conversion destinations in both hosted
and self-hosted workspaces. Connecting alone uploads nothing. Use [reviewed delivery](/guides/google-ads-preparation/) to validate and explicitly queue events.

## Connect

Open **Conversions → Connect Google Ads**. Allow the popup and grant Google Ads
and Data Manager permissions. Your workspace stays open while Google asks for
consent. If your browser isolates the popup, return to VisiConvert and choose
**Refresh Google connection**.

Only the workspace owner or self-hosted operator can connect Google. Scoped API
keys, the CLI and MCP do not manage provider credentials.

## Choose and check actions

1. Choose **Browse Google Ads**, then the account that gives you access. This can
   be a manager account.
2. Select or enter the advertising account ID, then **Load conversion actions**.
3. Select separate qualification and booking actions, or map just one stage.
   Only enabled import actions owned by this account are selectable.
4. Choose **Use selected actions**, then **Validate saved destination**.

For cross-account tracking, use the account that owns the conversion actions.
The UI shows the owning account when Google returns it. Discovery lists are
limited to 200 entries and show when results are truncated.

A successful check confirms readable account access and compatible enabled import
actions at the displayed time. It does not prove upload permission, event
acceptance or ad attribution. Changing the destination or reconnecting Google
invalidates the check. Uploaded conversion corrections and delivery diagnostics
belong to the next release.

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
