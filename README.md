<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/visiconvert-wordmark-light.svg" />
  <img src="assets/brand/visiconvert-wordmark.svg" alt="VisiConvert" width="340" />
</picture>

# VisiConvert

Self-hosted lead tracking and marketing attribution for service businesses.
Connect inquiries to qualified leads, bookings, and source revenue.

**Early beta, single workspace.** The application uses a Rust/Axum API,
PostgreSQL, React/Vite, and a shared Rust/WASM attribution preview. Hosting is in private beta;
VisiComms integration is planned. See [visiconvert.com](https://visiconvert.com) and the
[documentation](https://docs.visiconvert.com) for setup, the CLI, AI agents (MCP), and the API.

## Install

Requires Docker with the Compose plugin, Bash, and Python 3.11+. Allow roughly
4 GiB of available RAM and 20 GiB of disk for the initial source build.

```sh
bash scripts/self-host.sh init
bash scripts/self-host.sh up
```

Open http://127.0.0.1:3100. Retrieve your generated workspace key privately from
`.local/self-host/config.env`. No Visi account or subscription is needed.

See the [self-hosting guide](docs/SELF_HOSTING.md) for HTTPS, configuration,
upgrades, backups, and restoration. Carrier numbers and usage are separate
provider charges. You can explore form leads and bookings without a carrier.

## What works

- Start from a workspace setup checklist or connect an agent through local MCP. Hosted installations with the paired [remote MCP release](docs/REMOTE_MCP.md) also support a connector URL with VisiConvert sign-in and workspace approval. Configure sites/DNI with scoped keys and inspect real tracker, form and call evidence.

- Create or reuse workspace-specific Google Ads conversion actions from setup. New actions start as secondary; existing-action mapping remains available.

- [Prepare Google Ads conversions](docs/CONVERSIONS.md) from qualified inquiries and bookings, with consent evidence and reviewed local staging. Explicitly validate and queue Google delivery, with durable receipts and bounded retries.

- Connect Twilio or SignalWire per workspace with encrypted credentials and verified number ownership
  in both self-hosted and hosted modes. See [connection setup](docs/TWILIO_CONNECTIONS.md).
- Register existing customer-owned Twilio or SignalWire campaign numbers and configure
  forwarding destinations. Signed callbacks, deduplication, and call status
  handling are implemented; live carrier/fallback verification is outstanding.
- [Group repeat calls and forms into inquiries](docs/INQUIRY_GROUPING.md), with shared reviews, source evidence, booking totals, and separation/history controls.
- Capture website visits and confirmed successful forms with explicit consent, field
  allowlists, retry deduplication, and inspectable session source evidence.
  See [website setup](docs/WEBSITE_TRACKING.md). No automatic form listeners.
- Submit form leads through the API or application, qualify leads, and save notes.
- Create scoped workspace keys for the [public API, CLI, and MCP](docs/PUBLIC_API.md),
  with expiration, rotation, revocation, and management history.
- Record booked USD amounts in exact cents, revise or cancel outcomes, and
  report booking totals by source.
- Install with isolated PostgreSQL storage, generated credentials, explicit
  migrations, pre-upgrade backups, and empty-database restoration.

## Beta limitations

Self-hosted administration uses one operator key per workspace; integrations can use
separate scoped API keys. Local individual accounts,
roles, native form-platform connectors,
Microsoft conversion delivery, recordings, and SMS are not yet implemented.
Hosted billing is maintained in the separate Rails service. The application does not buy numbers or configure carrier routing for you.
Booking totals represent recorded business outcomes, not settled payments.

An optional [hosted account protocol](docs/HOSTED_PROTOCOL.md) supports separate
account services with tenant-scoped sessions and browser-bound access. It is
disabled by default and does not add a Cloud dependency to self-hosting.

Use for controlled evaluation while these capabilities mature. There is no
claim of full CallRail or WhatConverts feature parity.

## Verification and contributions

```sh
python3 -m unittest discover -s tests -v
python3 scripts/verify-self-host.py
```

The second command builds the app and exercises disposable installations,
restart persistence, backups, and restoration. It requires a working Docker
daemon and removes only its own test projects and volumes.

See [CONTRIBUTING.md](CONTRIBUTING.md), [release notes](CHANGELOG.md), and
[LICENSE](LICENSE) for contribution guidance, current scope, and license terms.

## License

AGPL-3.0-only, matching VisiGrid and VisiPass package metadata. See [LICENSE](LICENSE).
Third-party components retain their own licenses; the outlined wordmark uses
Manrope, whose font license is included in `assets/brand/FONT-LICENSE.txt`.

## Dynamic number insertion

Hosted and self-hosted workspaces can assign dedicated tracking numbers to consenting website sessions, with fallback, cooldown, and explicit inferred call-source evidence. See [setup and limits](docs/DYNAMIC_NUMBERS.md).

SignalWire setup: [connect an existing project and test a call](docs/SIGNALWIRE_CONNECTIONS.md).

Managed hosted service policies are enforced by the Rust engine; Rails retains
billing agreement history and issues versioned, expiring authorization. Failed
payments allow a seven-day forwarding grace, followed by suspension and a
45-day number hold. Scheduled cancellation keeps service through the paid
boundary. A fresh suspended policy is required for automatic release; expired
billing authorization alone cannot release numbers. Status callbacks continue
for final metering. BYOC connections remain outside managed billing enforcement.
The paired Cloud lifecycle feature is disabled until explicitly enabled during
rollout, and managed purchases remain gated. See the private Cloud billing
runbook for reconciliation and activation. Existing managed connections require
valid authorization before forwarding; deploy the matching Cloud worker before
enabling managed service.
