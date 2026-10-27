---
title: Introduction
description: What VisiConvert does and how the pieces fit together.
---

VisiConvert connects inquiries to results. Each tracking number belongs to one marketing source and campaign, so every call arrives labeled. You mark leads as qualified or not, record the jobs they book, and see booked revenue by source.

It is open source under AGPL-3.0 and built for US service businesses.

## Two ways to run it

- **Self-hosted.** Docker runs the app and a PostgreSQL database on your server. You bring your own Twilio or SignalWire account. See [Self-host VisiConvert](/start/self-host/).
- **Hosted.** The same engine, run for you. You still connect your own Twilio or SignalWire account, so numbers and call history stay yours.

## How a call is tracked

1. You buy a number from Twilio or SignalWire and register it in VisiConvert with a source (for example Google Ads), a campaign, and the business line it rings through to.
2. In the carrier console, you point that number's voice and status webhooks at your VisiConvert server.
3. When someone calls, the carrier asks VisiConvert what to do. VisiConvert records a lead with the number's source and campaign, then forwards the call.
4. Afterwards, you qualify the lead and record any booked job against it.

## Ways to work with it

- **The web app** for day-to-day review.
- **The [`visiconvert` command line](/cli/overview/)** for setup, scripts and bulk work.
- **The [MCP server](/agents/mcp/)** so an AI agent can check setup, qualify leads and record bookings.
- **The [public HTTP API](/api/public-api/)** for integrations using scoped workspace keys, including the CLI and MCP. The web app uses separate owner/operator endpoints. The public API and key-management screen are live for hosted workspaces and available in the current self-host source.

## Beta scope

Working today: call tracking on registered numbers, form and API leads, qualification and notes, bookings and cancellations, source reports, website tracking, dynamic number insertion, scoped API keys and the [v1 API](/api/public-api/). Google Ads conversions can be [prepared](/guides/google-ads-preparation/) but not sent yet. See [what each tool can do](/start/capabilities/). Setting webhook URLs on carrier numbers is still a manual step, and individual user accounts are not implemented on self-hosted installs yet.
