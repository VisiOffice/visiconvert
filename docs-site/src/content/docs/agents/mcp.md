---
title: Set up an AI agent
description: Connect Claude or another MCP client to VisiConvert so an agent can check setup, qualify leads and record bookings.
---

## Hosted: connect with a URL

The remote MCP feature requires the paired OAuth engine/Rails release and gateway update. Once your hosted installation has that release, add this custom connector URL:

```text
https://cloud.visiconvert.com/mcp
```

In Claude, choose **Register automatically** for the OAuth client if prompted. Sign in with your **VisiConvert** account, choose one workspace, and approve permissions. No CLI installation or API key is required. This release supports automatic registration, not Claude’s published client identity option.

Read access includes customer contact information. Write permissions start unchecked on the approval page. For website setup, select **Create and update websites and dynamic number pools** on VisiConvert’s approval page. Select **Register phone numbers you already own** if needed, and leave other write permissions unchecked unless you need them. These choices are available even if your client requests only read access; no manual OAuth scopes or API key are needed. To change an existing read-only connection, revoke it under **Account → Connected agents**, then reconnect from your agent and approve the setup permissions. The server only offers tools allowed by your approval. A workspace must be ready and have billing or complimentary access.

Revoke access anytime under **Account → Connected agents**. Connections expire after 90 days; password resets and loss of workspace access also block them. This is VisiConvert account authorization, independent of connecting Google Ads.

## Local: CLI and API key

`visiconvert mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io) server on stdio. It uses the same API and saved login as the command line. An agent can inspect setup evidence, configure websites and DNI pools, register tracking numbers, review leads and record bookings. The [tool reference](/reference/mcp/) lists every tool.

### Before you start

Open **Setup → Connect your agent** in your workspace. It provides a temporary-key preset, a login command, MCP configuration and a copyable setup prompt. These features require the engine and CLI containing migration **0012**; upgrade both together.

Install the CLI and log in. See [The visiconvert command line](/cli/overview/).
Use a separate absolute `VISICONVERT_CONFIG` path for each workspace, on the machine running the agent:

```sh
VISICONVERT_CONFIG=/home/you/.config/visiconvert/business.json visiconvert login --url https://cloud.visiconvert.com
```

Replace the path and server origin for your installation. Enter the API key at the private CLI prompt, never in agent chat. Read access includes customer contact information. For setup, grant `read` + `setup:write`; add `numbers:write` only to register owned numbers. The UI preset expires after seven days. Revoke it when finished.

In the MCP configuration, set the same `VISICONVERT_CONFIG` path and server origin. Clear any inherited `VISICONVERT_API_KEY` override (an empty value uses the saved key). Do not share one config file between workspaces. This alternative runs over local **stdio**; it does not use the remote MCP URL or OAuth.

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
      "args": ["mcp"],
      "env": {
        "VISICONVERT_CONFIG": "/home/you/.config/visiconvert/business.json",
        "VISICONVERT_URL": "https://cloud.visiconvert.com",
        "VISICONVERT_API_KEY": ""
      }
    }
  }
}
```

## What the agent can do

| Tier | Tools | When |
|---|---|---|
| **Read** | `get_status`, `get_setup_status`, `list_sites`, `get_site_pool`, `list_numbers`, `list_twilio_numbers`, `list_signalwire_numbers`, `list_leads`, `get_lead_outcomes`, `source_report` | Always |
| **Change** | `create_site`, `update_site`, `configure_site_pool`, `add_tracking_number`, `add_lead`, `qualify_lead`, `record_outcome` | With approved OAuth write scopes, or local `--allow-changes` plus key scopes |

Every tool carries MCP annotations that mark it as read-only or not, so clients can ask before changes. For local stdio, without `--allow-changes` the change tools are not listed. Remote MCP independently filters tools by the scopes you approved.

## What it will not do

- **Handle provider credentials.** No tool accepts them. Connect Twilio or SignalWire yourself in the workspace UI (or with the legacy self-hosted operator CLI), so your provider secrets never enter the agent's context.
- **Buy or release numbers**, or delete leads and bookings.

## Guided setup

The server includes a prompt, `set_up_call_tracking`. In clients that support MCP prompts, it walks the agent through checking status, choosing websites and campaigns, configuring consent/form hooks and DNI pools, and confirming real visits, forms and test calls. It asks you before each change and tells you to connect the provider yourself. If your client exposes tools but not MCP prompts, paste the setup prompt from **Setup → Connect your agent** instead. It contains the workflow; unavailable MCP prompts do not block setup.

You can also just ask, for example:

> Set up call tracking for Google Ads, Facebook and our yard signs. Calls should ring through to our main line.

## Current coverage

The tools cover sites, DNI pools, numbers, individual interactions, qualification, bookings and source reports. An agent with access to your website repository can install the tracker; MCP itself cannot edit your website. Inquiry combine/split, conversion preparation, API-key administration and provider credentials still require the workspace UI.

Creating a site is not retry-safe: list sites after an uncertain response before retrying. Site and pool updates require the current version; a conflict means re-read and review, not blind retry. Changing a site's origin or pausing collection revokes active assignments. Pool numbers must be dedicated to one site's DNI and share the business fallback destination. Enabling `--allow-changes` exposes all write tools, but the key's permissions remain authoritative.

**Setup checklist** reports historical activity separately for each site and number. Configuration alone does not verify installation or forwarding. Form activity is reported by the website and must be wired to actual backend success. After a real call, verify the destination rings and inspect attribution evidence in the inbox. Do not create fake leads or count widget opens to make setup appear complete. Carrier routing changes and purchases need explicit owner approval and are not available through these MCP tools. Google Ads uploads are still disabled.

Use `visiconvert setup` for the same evidence from your terminal. Reviewing a grouped
interaction updates its inquiry's shared qualification and notes. Read access exposes
customer contacts and notes to the connected agent; choose an appropriate client.

Hosted CLI/MCP access uses [workspace API keys](/api/public-api/).
The browser's short-lived `vc_session_` token is not an automation credential.
For desktop clients that cannot find `visiconvert`, use its absolute installed path
in `command` and ensure the process has the same config path/environment as your shell.
