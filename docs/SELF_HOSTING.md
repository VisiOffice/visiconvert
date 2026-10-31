# Self-host VisiConvert

This is a single-workspace beta. Docker runs the Rust API, bundled web/WASM app,
and a dedicated PostgreSQL database. No VisiComms account, hosted subscription,
or carrier account is needed to install and explore form leads and bookings.
VisiConvert is licensed under AGPL-3.0-only; see `LICENSE`. Installations build
from source; no public container registry image is supplied.

## Requirements

- Linux or macOS with Docker Engine/Desktop and the Docker Compose plugin.
- Bash and Python 3.11 or newer. Rust, Node, pnpm, and PostgreSQL are built/run
  inside Docker; they are not required on the host.
- Internet access for official base images, Rust crates, and npm packages.
- Allow roughly 4 GiB available RAM and 20 GiB free disk for a source build.
  The app and database have runtime memory limits of 512 MiB and 768 MiB.
  The first build can take several minutes; later builds reuse Docker's cache.
- An unused host port (default `3100`). The app listens on host loopback only.

## Install

From the extracted source directory:

```sh
bash scripts/self-host.sh init
bash scripts/self-host.sh up
```

Open **http://127.0.0.1:3100**. Retrieve `VISICONVERT_API_KEY` privately from
`.local/self-host/config.env` and enter it on the login screen. The installer
never prints the key. The app holds it in browser memory for the session.

`init` creates independent random database, workspace login, and provider-encryption keys, a workspace UUID,
and mode-600 configuration. Repeating it preserves the existing file.
`up` builds a uniquely tagged production image, waits for the database, backs it
up, applies migrations, starts the app, and checks the frontend, authenticated
database API, callback URLs, and rejection of anonymous requests. Demo leads are
not seeded automatically. Start by adding a form lead in the app.

The default Compose project is `visiconvert-selfhost`. Its data lives in the
Docker volume `visiconvert-selfhost_postgres_data`. The database has no published
host port. Existing pilot deployments use a separate project and configuration.

For a different port or domain, supply these options during the first init:

```sh
bash scripts/self-host.sh init --port 43110 --origin https://leads.example.com
```

For multiple installations or to keep configuration outside the source directory,
give each installation a unique directory, project name, and host port:

```sh
bash scripts/self-host.sh --directory /path/to/private/visiconvert init \
  --project visiconvert-team --port 43110
bash scripts/self-host.sh --directory /path/to/private/visiconvert up
```

Use the same `--directory` for every subsequent command. Do not change the project
name or workspace UUID on an existing installation; they identify its data.
Never regenerate the database password on an initialized volume: changing an
environment variable does not change PostgreSQL's existing role password.

## Configuration and domain access

Edit `config.env` with unquoted `KEY=value` entries, then run `up` to apply changes.
The file is data, not a shell script. Unknown keys, duplicate keys, shell
interpolation, weak generated-key replacements, and invalid origins are rejected.
Shell exports and the repository's development `.env` do not override this file.

| Setting | Purpose |
| --- | --- |
| `COMPOSE_PROJECT_NAME` | Stable installation name; determines the Docker volume |
| `VISICONVERT_HOST_PORT` | Loopback HTTP port, 1024–65535 |
| `PUBLIC_URL` | External origin, without a trailing slash/path, used in callback URLs |
| `VISICONVERT_API_KEY` | 64-character hex workspace login key |
| `VISICONVERT_ENCRYPTION_KEY` | Stable 64-character hex key encrypting saved provider credentials; keep with backups |
| `VISICONVERT_POSTGRES_PASSWORD` | 64-character hex database password |
| `VISICONVERT_WORKSPACE_ID` | Stable UUID identifying the workspace |
| `TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN` | Both empty by default |

For a remote server, use an SSH tunnel for initial access:

```sh
ssh -N -L 3100:127.0.0.1:3100 user@your-server
```

For public access, point your existing HTTPS reverse proxy or Cloudflare Tunnel
at `http://127.0.0.1:3100` (or the configured port), and set `PUBLIC_URL` to your
HTTPS origin. A proxy inside another container needs network connectivity to
the app; its own loopback does not reach the host. The installer does not manage
DNS, TLS, firewalls, or take over ports 80/443. The marketing site is separate.

The verification command checks the published local port. After setting up your
domain, also visit it over HTTPS and log in to confirm the proxy path works.

## Routine operations

```sh
bash scripts/self-host.sh status
bash scripts/self-host.sh verify
bash scripts/self-host.sh logs
bash scripts/self-host.sh backup
bash scripts/self-host.sh stop
bash scripts/self-host.sh up
```

`stop` preserves the database and configuration. `logs` shows the last 100 lines;
review logs before sharing them, since server errors may contain operational data.
An exclusive lock prevents concurrent management commands for one configuration
directory. Do not run direct Compose mutations concurrently with the helper.

## Upgrades

1. Keep `.local/self-host/` intact, or continue using your external configuration
   directory. Copy in the new source version without deleting your configuration.
2. Run `bash scripts/self-host.sh up`.
3. Confirm verification passes and check your existing leads and bookings.

The build finishes before the running app is stopped. With writes stopped, the
helper makes a custom-format database backup and saves the configuration and
previous image reference. It runs migrations explicitly on every upgrade instead
of trusting an old successful migration container. The app restarts only after
migrations succeed. Brief downtime is expected during backup/migration/startup.

Backups are under `.local/self-host/backups/<timestamp>/`. A completed backup has
`database.dump`, `config.env`, and `manifest.json` with its SHA-256 checksum,
workspace ID, and image reference. A failed backup retains an `.incomplete`
directory and stops the upgrade. A clean first install also backs up its empty
database. Old images/backups are retained; no pruning or retention policy runs.

Protect backups as private customer data: the configuration copy contains keys.
Copy completed backups to another machine/storage service and schedule `backup`
using your own scheduler. Backups on the same disk are not disaster recovery.

## Restore and rollback

Restore is intentionally limited to an **empty database**, so it cannot overwrite
an existing installation. Use a new project, port, and configuration directory:

```sh
bash scripts/self-host.sh --directory /path/to/recovery init \
  --project visiconvert-recovery --port 43111
bash scripts/self-host.sh --directory /path/to/recovery restore \
  /path/to/completed-backup
bash scripts/self-host.sh --directory /path/to/recovery up
```

The restore verifies the archive checksum and restores within one transaction.
It keeps the recovery installation's new database password, API key, and origin,
but adopts the backed-up workspace UUID and provider-encryption key so the
original leads and encrypted carrier credentials remain usable.
After `up`, log in using the recovery configuration's key and verify your data.

For a rollback, use the matching old source version or load the retained image
named in the backup manifest, then run:

```sh
bash scripts/self-host.sh --directory /path/to/recovery up \
  --image visiconvert-selfhost:YOUR-RETAINED-TAG
```

`--image` requires an already-loaded local image; it never silently pulls a new
version. To move an image to another host, use Docker's `save`/`load` commands.
Verify recovery before switching your reverse proxy to the recovered instance.
There is no automatic rollback across database schema changes. Never run
`docker compose down -v` on an installation containing data you need.

## Beta scope

Working: form lead entry/API ingestion, qualification, notes, bookings,
cancellations, source reporting, and tracking-number configuration. This is one
workspace with a shared operator key; individual users, permissions, and public
signup are not implemented. Dynamic number insertion and automatic website form
capture are still planned.

Twilio starts disconnected. Use the shared **Connections** screen to verify your
account and register owned voice numbers. Existing installations can enable
credential storage with `bash scripts/self-host.sh enable-connections`, then
restart with `up`. See [Twilio connections](TWILIO_CONNECTIONS.md), including
the explicit cutover procedure for legacy environment credentials.

Real calls require your carrier account, tracking numbers,
HTTPS, webhook configuration, and a verified forwarding destination. Adding a
number in the app does not buy or configure it at Twilio. Validate signed live
callbacks and fallback handling before routing customer calls through the beta.

## Troubleshooting

- **Docker unavailable:** install/start Docker and ensure your account can use it.
- **Port already allocated:** choose another port in `config.env`; update the
  local `PUBLIC_URL` too if it contains the old port, then run `up` again.
- **Migration/startup failure:** run `logs`; backups and previous images remain.
  Resolve the reported error and retry `up`, or restore into a recovery project.
- **Workspace mismatch:** restore the original UUID from a configuration backup.
- **Login denied:** use this installation's `VISICONVERT_API_KEY`, not the
  development `.env` or another deployment's key.
- **Remote browser cannot connect:** use an SSH tunnel or configure HTTPS ingress;
  the default port is deliberately bound to loopback.
- **Restore refused:** choose a new project with an empty database. The helper
  will not clear a populated database for you.

Compose health/dependency behavior follows the
[Docker startup-order documentation](https://docs.docker.com/compose/how-tos/startup-order/).
The volume layout uses the [official PostgreSQL 17 image](https://hub.docker.com/_/postgres).

## Creating a transferable source bundle

From the development checkout, run `python3 scripts/package-self-host.py`. It
creates a source archive and SHA-256 file under `artifacts/self-host/`, with an
explicit allowlist that excludes local configuration, backups, and dependencies.
Transfer both files, verify with `sha256sum -c <archive>.sha256`, and
extract the archive. Run the install commands inside its `visiconvert/` directory.
Use `--release` to require license metadata. Building a bundle does not publish it.

## Optional Google connection

Google OAuth, account/action discovery and destination checks work in self-hosted
installs. Follow [Google connection setup](GOOGLE_CONNECTION.md) to configure your
own Web OAuth client in private `config.env`. Both client variables are optional;
set them together with the existing encryption key. Delivery defaults to paused; see [reviewed Google delivery](CONVERSIONS.md) to validate and explicitly queue events.
