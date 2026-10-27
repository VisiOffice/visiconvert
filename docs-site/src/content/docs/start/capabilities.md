---
title: What each tool can do
description: Which tasks work in the web app, the CLI, the MCP server and the public API.
---

The web app does everything. The command line, MCP server and public API cover the day-to-day work an integration or agent needs. Setup that changes how calls or websites are tracked stays in the web app on purpose.

| Task | Web app | CLI | MCP | API v1 |
|---|:-:|:-:|:-:|:-:|
| See Twilio and SignalWire status | ✓ | ✓ | ✓ | ✓ |
| Connect SignalWire | ✓ | — | — | — |
| Connect Twilio | ✓ | Self-hosted operator key only | — | — |
| List tracking numbers and untracked provider numbers | ✓ | ✓ | ✓ | ✓ |
| Register a tracking number | ✓ | ✓ | ✓ | ✓ |
| List leads (interactions) | ✓ | ✓ | ✓ | ✓ |
| List inquiries | ✓ | — | — | ✓ |
| Add a form or manual lead | ✓ | ✓ | ✓ | ✓ |
| Qualify and add notes | ✓ | ✓ | ✓ | ✓ |
| Record, revise or cancel a booking | ✓ | ✓ | ✓ | ✓ |
| Source report | ✓ | ✓ | ✓ | ✓ |
| Combine or separate inquiries | ✓ | — | — | — |
| Set up website tracking (Sites) | ✓ | — | — | — |
| Set up dynamic number insertion | ✓ | — | — | — |
| Connect Google and validate destination actions | ✓ | — | — | — |
| Prepare Google Ads conversions | ✓ | — | — | — |
| Create, rotate or revoke API keys | ✓ | — | — | — |

What a CLI or MCP session can change also depends on the key it logged in with. A read-only [API key](/api/public-api/) stays read-only, even if the MCP server is started with `--allow-changes`.

Not available anywhere yet: buying numbers, setting carrier webhooks automatically, call recording, SMS, and sending conversions to Google.
