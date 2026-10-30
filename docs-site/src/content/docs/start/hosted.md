---
title: Hosted accounts and workspaces
description: Sign in, open a business workspace, manage billing and recover access.
---

The unified hosted interface keeps your account, workspaces and billing at
`cloud.visiconvert.com`, using the same interface as the workspace app.
Registration is open. Billing currently uses Stripe test mode.

## Get into a workspace

1. Sign in with your email and password. If registration is open, choose
   **Create account**, then verify your email before creating a workspace.
2. Choose **New workspace** and enter the business name. Each workspace keeps
   its inquiries, phone numbers, reporting and subscription separate.
3. Complete billing if required, then choose **Open workspace** after setup
   finishes. Existing pilot workspaces may have access without a subscription.

You can [create an account](https://cloud.visiconvert.com/registration/new) or
[sign in](https://cloud.visiconvert.com/session/new). Self-hosted installations
use their own operator key and do not need a hosted account.

## Billing and account navigation

Choose **Workspaces & billing** from inside a workspace to return to your account.
Open a workspace's **Billing** page to view its status. Subscription checkout
and payment-method management open Stripe's secure pages. Each subscription
covers one workspace; phone numbers and carrier usage are separate.

Billing currently runs in test mode. The interface labels test billing; it does
not collect a real payment. Public launch and live subscriptions are separate
from the interface rollout.

## Managed call service

Managed-number billing is still gated. When enabled, failed payments block new
purchases immediately while existing calls have a seven-day payment grace period.
Forwarding then pauses and numbers remain held for 45 more days before release.
Scheduled cancellation keeps calls working through the paid period, followed by
a 45-day number hold. The Billing page shows the applied service state and hold
deadline separately from a change still waiting to reach the workspace.

Restore billing before carrier release begins to retain the numbers. A new
subscription does not erase earlier unpaid usage or invoices. Numbers you have
separately requested to release stay in that release process. Bring-your-own
Twilio and SignalWire accounts are not enrolled in managed billing by these rules.

## Sessions and recovery

Reloading an open workspace signs it back in through your account. If a workspace
session expires, return to the workspace list and open it again. A failed write
is not automatically retried; check its result before entering it again.

**Sign out** closes your account session and the current workspace session.
Other tabs or devices may retain their separate workspace sessions until expiry.
Use **Forgot password?** to request a reset email. Password reset revokes existing
workspace credentials; during a temporary service outage it can remain pending
while that revocation completes.

## Integrations

API keys are managed inside each workspace under **API keys**. They are separate
from your browser session. Existing CLI, MCP and HTTP integrations can continue
using `https://workspaces.visiconvert.com`; see [API keys](/api/public-api/).
Keep using the webhook URLs for your provider displayed under **Connections**.
