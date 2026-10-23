---
title: The visiconvert command line
description: Install the CLI, log in, and use it for setup, lead review and bookings.
---

`visiconvert` works against a self-hosted server or a hosted workspace. It uses the same [HTTP API](/api/overview/) as the web app. Every command and option is listed in the [command reference](/reference/cli/).

## Install

Build it from the source repository. You need a Rust toolchain:

```sh
cargo install --path crates/convert-cli
```

## Log in

```sh
visiconvert login --url https://convert.example.com
```

It prompts for the workspace key and checks it against the server. On a self-hosted install, the key is `VISICONVERT_API_KEY` in `config.env`. The URL must use HTTPS unless it is `localhost`.

The key is saved to `~/.config/visiconvert/config.json` with mode 0600. For scripts and CI, use environment variables instead:

| Variable | Purpose |
|---|---|
| `VISICONVERT_URL` | Server URL; overrides the saved one |
| `VISICONVERT_API_KEY` | Workspace key; overrides the saved one |
| `VISICONVERT_CONFIG` | Path to a different config file |

## Set up tracking

```sh
visiconvert status
visiconvert connect twilio --account-sid AC…
visiconvert numbers available
visiconvert numbers add +15125550142 --forward-to +15125550100 \
  --source "Google Ads" --campaign "Spring HVAC"
```

`status` shows the server, the Twilio connection and what to do next. After `numbers add`, set the printed webhook URLs on the number in Twilio. See [Connect Twilio](/start/connect-twilio/).

## Review leads and record bookings

```sh
visiconvert leads list --since 7d --status unreviewed
visiconvert leads qualify 9ec1ff6e qualified --notes "Wants a quote for two units"
visiconvert leads outcome 9ec1ff6e --ref JOB-1042 --booked 1250.00
visiconvert report sources
```

Lead IDs can be shortened to any unique prefix of six or more characters. See [Leads, qualification and bookings](/concepts/leads-and-bookings/) for how versions and revisions work.

## Scripting

Add `--json` to any command for machine-readable output. Errors exit with status 1 and, with `--json`, print `{"error": "…"}` to stdout.

```sh
visiconvert leads list --since 24h --json | jq -r '.[] | select(.qualification == "unreviewed") | .id'
```
