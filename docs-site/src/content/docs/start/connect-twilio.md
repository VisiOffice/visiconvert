---
title: Connect Twilio
description: Connect your Twilio account, register tracking numbers and point their webhooks at VisiConvert.
---

VisiConvert uses numbers in your own Twilio account. It verifies the account and that you own each number. It does not buy numbers or change their settings in Twilio yet.

## Before you start

- Your server has an HTTPS `PUBLIC_URL`. Twilio will not call a plain-HTTP or loopback address.
- `VISICONVERT_ENCRYPTION_KEY` is set. New self-hosted installs generate it. Older installs can add it with `bash scripts/self-host.sh enable-connections` followed by `up`.
- You have at least one voice-capable +1 number in Twilio.

## 1. Connect the account

In the web app, open **Connections**, enter the Account SID and Auth Token, and choose **Verify and connect Twilio**. For a subaccount, use its own SID and token.

Or from the command line:

```sh
visiconvert connect twilio --account-sid AC…
```

It prompts for the Auth Token without echoing it. To pipe it in from a secret manager, add `--token-stdin`. The token is encrypted with AES-256-GCM and never returned by the API.

## 2. Register tracking numbers

Each number gets a source, a campaign and the business line it forwards to:

```sh
visiconvert numbers available
visiconvert numbers add +15125550142 --forward-to +15125550100 \
  --source "Google Ads" --campaign "Spring HVAC"
```

VisiConvert checks with Twilio that the number is in your account and can take voice calls.

## 3. Set the webhooks in Twilio

This step is manual for now. `numbers add` and the **Connections** screen show two URLs for the workspace. In the Twilio console, open each tracking number and set:

| Twilio field | Value | Method |
|---|---|---|
| A call comes in | The voice URL | HTTP POST |
| Call status changes | The status URL | HTTP POST |

## 4. Test a real call

Call the tracking number from your own phone. Check that the business line rings and that the lead appears with the right source:

```sh
visiconvert leads list --since 15m
```

:::caution
Before you send customer calls through the beta, set a fallback URL on each number in Twilio and test it with VisiConvert stopped. That way calls still reach you if your server is down.
:::

## Changing or disconnecting

To rotate the token, submit the same Account SID with the new token. An account cannot be changed or disconnected while it still has tracking numbers registered.
