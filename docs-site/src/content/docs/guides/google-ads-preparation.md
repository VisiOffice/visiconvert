---
title: Google Ads conversion preparation
description: Map qualified leads and bookings to Google Ads conversion actions and review them before sending is available.
---

:::caution[Preview]
Nothing is sent to Google yet. This release only prepares and reviews conversions. Connecting a Google Ads account and uploading are the next step.
:::

The goal is to tell Google Ads which clicks became qualified leads and booked jobs, so it can bid towards revenue rather than just calls.

## What you can do now

1. Open **Conversions**, enter your Google Ads customer ID, and map qualified leads and bookings to conversion actions.
2. Review each candidate: its click ID, the time it was recorded and its source.
3. Record the visitor's advertising consent, if you have evidence of it. Website tracking consent does not count.
4. Stage the conversion. Staged conversions are saved with a stable ID, so they will not be counted twice once sending is added.

## What makes a lead eligible

- It came from a website visit with a Google click ID (`gclid`, `gbraid` or `wbraid`).
- Advertising consent was recorded as granted.
- It is qualified, or has a booking that isn't canceled.
- It happened within 30 days of the visit, and you're reviewing it within 30 days.

Phone calls are eligible only if they were matched to a visit by [dynamic number insertion](/guides/dynamic-numbers/) and you turn that on.

If a booking, qualification, grouping or consent changes later, staged conversions are marked for review again.

Full technical reference: [CONVERSIONS.md](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/CONVERSIONS.md).
