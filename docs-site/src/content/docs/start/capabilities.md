---
title: What each tool can do
description: Which tasks work in the web app, the CLI, the MCP server and the public API.
---

The web app provides guided setup and daily lead management. MCP and the public API can also configure websites and DNI with explicit setup permission. Provider credentials and key administration stay in the web app. The setup tools below require engine migration 0012 and a matching CLI.

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
| Configure website tracking (Sites) | ✓ | — | ✓ | ✓ |
| Configure dynamic number insertion | ✓ | — | ✓ | ✓ |
| Inspect setup activity per site and number | ✓ | ✓ | ✓ | ✓ |
| Connect Google and validate destination actions | ✓ | — | — | — |
| Prepare Google Ads conversions | ✓ | — | — | — |
| Create, rotate or revoke API keys | ✓ | — | — | — |

What a CLI or MCP session can change also depends on the key it logged in with. A read-only [API key](/api/public-api/) stays read-only, even if the MCP server is started with `--allow-changes`.

Not available anywhere yet: buying numbers, setting carrier webhooks automatically, call recording, SMS, and sending conversions to Google.
