---
title: Troubleshooting
description: Fixes for common problems with calls, website tracking, the CLI and the API.
---

## Calls

**Calls don't show up as leads.** Check that the number's voice and status webhooks in Twilio match the URLs in **Connections**, both with HTTP POST, and that `PUBLIC_URL` is your HTTPS address. Then call again and run `visiconvert leads list --since 15m`.

**The business line doesn't ring.** Check the number's forwarding destination in **Tracking numbers**. It must be a +1 number.

**A call shows as "Unknown / pooled call".** The number is in a DNI pool and no visitor had it at the time of the call. See [how sure the match is](/guides/dynamic-numbers/#how-sure-the-match-is).

## Website tracking

**Sites shows no sessions.** Check, in order: the site is enabled, the origin matches exactly (including `www`), `setConsent(true)` was called, your CSP allows the VisiConvert origin, and no ad blocker is stopping it. The browser's network panel shows the request and its error.

**Forms don't appear.** `trackForm` has to be called after your backend confirms success, with a `submissionId`. Visiting a thank-you page is not enough.

**The phone number doesn't change.** Check `data-dni="true"` on the script, `data-visiconvert-phone` on the element, that the element contains only the number, and that the number matches the fallback exactly.

## CLI, MCP and API

**401 Unauthorized.** The key is wrong, expired or revoked. Create a new one under **API keys** and run `visiconvert login` again.

**403 Forbidden.** The key doesn't have the permission for this. For example, recording a booking needs `bookings:write`. On hosted workspaces, it can also mean the subscription isn't active.

**409 Conflict.** Someone changed the lead or booking since you read it. Read it again and retry. The CLI reads the current version on every run, so running the command again usually works.

**429 Too many requests.** Each key allows 120 requests a minute. Wait 60 seconds.

**The CLI won't accept the URL.** It must be `https://`, except for `localhost`, `127.0.0.1` or `[::1]`, with no path. Use the server's origin, such as `https://workspaces.visiconvert.com`, not the billing site.

**The agent can't make changes.** Start the MCP server with `--allow-changes`, and log in with a key that has the write permissions it needs.
