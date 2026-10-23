---
title: Set up an AI agent
description: Connect Claude or another MCP client to VisiConvert so an agent can check setup, qualify leads and record bookings.
---

`visiconvert mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io) server on stdio. It offers the same operations as the command line, so an agent can check your setup, register tracking numbers, review leads and record bookings. The [tool reference](/reference/mcp/) lists every tool.

## Before you start

Install the CLI and log in. The MCP server uses the same saved login. See [The visiconvert command line](/cli/overview/).

## Claude Code

```sh
claude mcp add visiconvert -- visiconvert mcp
```

Add `--allow-changes` only when the agent needs to write. Its API key must separately grant the matching scopes; enabling tools does not elevate the key.

## Claude Desktop and other clients

Add the server to the client's MCP configuration. For Claude Desktop, that is `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "visiconvert": {
      "command": "visiconvert",
      "args": ["mcp"]
    }
  }
}
```

## What the agent can do

| Tier | Tools | When |
|---|---|---|
| **Read** | `get_status`, `list_numbers`, `list_twilio_numbers`, `list_leads`, `get_lead_outcomes`, `source_report` | Always |
| **Change** | `add_tracking_number`, `add_lead`, `qualify_lead`, `record_outcome` | Only with `--allow-changes` |

Every tool carries MCP annotations that mark it as read-only or not, so clients can ask before changes. Without `--allow-changes`, the change tools are not listed at all.

## What it will not do

- **Handle Twilio credentials.** No tool accepts them. Connect Twilio yourself in the workspace UI (or with the legacy self-hosted operator CLI), so your Auth Token never enters the agent's context.
- **Buy or release numbers**, or delete leads and bookings.

## Guided setup

The server includes a prompt, `set_up_call_tracking`. In clients that support MCP prompts, it walks the agent through checking status, choosing which campaigns to track, assigning numbers and confirming a test call. It asks you before each change and tells you to connect Twilio yourself.

You can also just ask, for example:

> Set up call tracking for Google Ads, Facebook and our yard signs. Calls should ring through to our main line.

## Current coverage

The tools cover numbers, individual interactions, qualification, bookings and source
reports. Site installation, DNI pools, inquiry combine/split, conversion preparation,
and API-key administration currently require the workspace UI. Reviewing a grouped
interaction updates its inquiry's shared qualification and notes. Read access exposes
customer contacts and notes to the connected agent; choose an appropriate client.

Hosted CLI/MCP access uses [workspace API keys](/api/public-api/) after rollout.
The browser's short-lived `vc_session_` token is not an automation credential.
For desktop clients that cannot find `visiconvert`, use its absolute installed path
in `command` and ensure the process has the same config path/environment as your shell.
