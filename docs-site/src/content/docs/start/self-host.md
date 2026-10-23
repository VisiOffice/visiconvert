---
title: Self-host VisiConvert
description: Install the app and database with Docker, then keep them backed up and upgraded.
---

A self-hosted install is one workspace. Docker runs the Rust API, the web app and a dedicated PostgreSQL database. You do not need a hosted subscription or a Twilio account to install it and try form leads and bookings.

## Requirements

- Linux or macOS with Docker and the Docker Compose plugin.
- Bash and Python 3.11 or newer. Rust, Node and PostgreSQL run inside Docker.
- About 4 GiB of free RAM and 20 GiB of disk for the first build.
- A free host port. The default is `3100`, bound to loopback only.

## Install

Download the [current patched API source](https://github.com/VisiOffice/visiconvert/archive/6b76432bc1fde9686266cd5ec03449235d2a9685.tar.gz), then from the source directory:

```sh
bash scripts/self-host.sh init
bash scripts/self-host.sh up
```

Open `http://127.0.0.1:3100`. The login key is `VISICONVERT_API_KEY` in `.local/self-host/config.env`. The installer never prints it.

`init` generates the database password, workspace key and encryption key, and writes them to a mode-600 file. `up` builds the image, backs up the database, runs migrations, starts the app and checks it.

To use a different port or a public domain, pass them to the first `init`:

```sh
bash scripts/self-host.sh init --port 43110 --origin https://leads.example.com
```

## Make it reachable

Twilio has to reach your server over HTTPS. Point a reverse proxy or a Cloudflare Tunnel at `http://127.0.0.1:3100` and set `PUBLIC_URL` in `config.env` to the HTTPS origin, then run `up` again. The installer does not manage DNS, TLS or firewalls.

For first access to a remote server, an SSH tunnel is enough:

```sh
ssh -N -L 3100:127.0.0.1:3100 user@your-server
```

## Everyday commands

```sh
bash scripts/self-host.sh status
bash scripts/self-host.sh verify
bash scripts/self-host.sh logs
bash scripts/self-host.sh backup
bash scripts/self-host.sh stop
```

## Upgrade

Copy in the new source without deleting `.local/self-host/`, then run `bash scripts/self-host.sh up`. It backs up the database before migrating, and only restarts the app after migrations succeed.

## Back up

Backups go to `.local/self-host/backups/<timestamp>/`. Each has the database dump, a copy of the configuration and a checksum manifest.

:::caution
A backup includes your keys, including `VISICONVERT_ENCRYPTION_KEY`. Without that key, saved Twilio credentials cannot be decrypted. Copy backups off the machine and keep them private.
:::

Restores only go into an empty installation, so they can never overwrite live data. The full procedure, along with rollback and troubleshooting, is in the [self-hosting guide on GitHub](https://github.com/VisiOffice/visiconvert/blob/6b76432bc1fde9686266cd5ec03449235d2a9685/docs/SELF_HOSTING.md).
