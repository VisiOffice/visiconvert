---
title: The visiconvert command line
description: Install the CLI, log in, and use it for setup, lead review and bookings.
---

`visiconvert` works against a self-hosted server or a hosted workspace. Scoped workspace keys use the [public HTTP API](/api/public-api/). Every command and option is listed in the [command reference](/reference/cli/).

## Install

Build the version matching the deployed API. You need Git and a current Rust toolchain:

```sh
git clone https://github.com/VisiOffice/visiconvert.git
cd visiconvert
git checkout 6b76432bc1fde9686266cd5ec03449235d2a9685
cargo install --locked --path crates/convert-cli
```

## Create a workspace API key

The public API and key screen are live for hosted workspaces. Self-hosted users
should upgrade the engine and CLI together. See [API keys and v1](/api/public-api/).

Open your workspace, choose **API keys**, and create a named key. Read-only is the
default. Grant write permissions only when needed. Save the secret when shown;
you cannot retrieve it later. Hosted keys do not expire with the browser session.
Use the engine origin (`https://workspaces.visiconvert.com` for hosted).
Existing CLI configurations continue to work with the unified hosted app. The same workflow is available in self-hosted workspaces.

## Log in

```sh
visiconvert login --url https://convert.example.com
```

It prompts for the workspace key and checks it against the server. Use the new `vc_key_…` secret. An existing self-hosted operator key from `config.env` still works in legacy mode, with unrestricted administration access. The URL must use HTTPS unless it is `localhost`.

The key is saved to `~/.config/visiconvert/config.json` with mode 0600. For scripts and CI, use environment variables instead:

| Variable | Purpose |
|---|---|
| `VISICONVERT_URL` | Server URL; overrides the saved one |
| `VISICONVERT_API_KEY` | Workspace key; overrides the saved one |
| `VISICONVERT_CONFIG` | Path to a different config file |

## Set up tracking

```sh
visiconvert status
visiconvert numbers available
visiconvert numbers add +15125550142 --forward-to +15125550100 \
  --source "Google Ads" --campaign "Spring HVAC"
```

Connect Twilio in the workspace UI; scoped keys cannot change provider credentials. Number registration requires `numbers:write`.

`status` shows the server, the Twilio connection and what to do next. After `numbers add`, set the printed webhook URLs on the number in Twilio. See [Connect Twilio](/start/connect-twilio/).

## Review leads and record bookings

```sh
visiconvert leads list --since 7d --status unreviewed
visiconvert leads qualify 9ec1ff6e qualified --notes "Wants a quote for two units"
visiconvert leads outcome 9ec1ff6e --ref JOB-1042 --booked 1250.00
visiconvert report sources
```

Lead IDs can be shortened to any unique prefix of six or more characters. See [Leads, qualification and bookings](/concepts/leads-and-bookings/) for how versions and revisions work.

With scoped keys, the CLI follows v1 pagination; a full interaction UUID is fetched
directly. Legacy operator-key mode retains the 500-interaction server cap. Reviewing
one interaction updates its entire inquiry. Bookings stay on their original
interaction. API scopes also apply to MCP, even with `--allow-changes`.

## Scripting

Add `--json` to any command for machine-readable output. Operational errors exit with status 1 and, with `--json`, print `{"error": "…"}` to stdout.

```sh
visiconvert leads list --since 24h --json | jq -r '.[] | select(.qualification == "unreviewed") | .id'
```
