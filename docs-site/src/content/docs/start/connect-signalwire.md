---
title: Connect SignalWire
description: Connect an existing SignalWire project and verify call tracking.
---

SignalWire call tracking runs in the shared Rust engine for hosted and self-hosted workspaces. It uses the Compatibility API and cXML voice webhooks. Twilio and SignalWire can coexist in a workspace; every registered number has an explicit provider. Existing registrations remain Twilio.

## Connect an existing project

1. Configure an HTTPS `PUBLIC_URL` and retain `VISICONVERT_ENCRYPTION_KEY` in backups. New self-host installations generate the key; older installations can run `bash scripts/self-host.sh enable-connections`, then `up`.
2. Open **Connections → SignalWire** as the workspace owner (or self-hosted operator).
3. Enter the Space hostname, such as `your-space.signalwire.com`, the Project ID, an API token with access to account details and incoming number inventory, and the project's separate webhook signing key. Account retrieval requires Management scope. Use a dedicated project for each business and keep secrets out of chat, command arguments and source control.
4. Choose **Verify and connect SignalWire**. This checks account status and inventory access. The signing key is verified only when a signed callback arrives; an account connection is not proof of successful forwarding.
5. In **Tracking numbers**, select SignalWire, register a voice-capable US number already in that project, and choose its destination, source and campaign. Ownership is verified before saving. Numbers cannot forward to another registered tracking number.

Both secrets are encrypted with AES-256-GCM, bound to the workspace, Space, project, and credential purpose. APIs never return them. Rotation requires the current revision and clears callback readiness. Registered numbers prevent disconnecting or changing the project; rotating secrets for the same project remains possible.

## Configure and test one number

Connecting does not buy, release, or reconfigure numbers. Save the old routing before changing a number in SignalWire. If it currently uses Varda, SWML or an AI handler, replacing that handler changes the answering path.

Configure cXML handling with the displayed **incoming voice** and **call status** URLs, both HTTP POST:

```
https://YOUR_ENGINE/webhooks/signalwire/workspaces/WORKSPACE_ID/voice
https://YOUR_ENGINE/webhooks/signalwire/workspaces/WORKSPACE_ID/status
```

Use the displayed engine URLs; do not construct them from the billing/marketing domain. Keep the paths and query-free URLs exact. VisiConvert returns `<Dial>` with a signed completion callback and recording explicitly disabled.

Place a real call, answer at the forwarding destination, hang up and inspect the inquiry. Check the source/campaign, destination result and duration. “Destination answered” can mean voicemail or an IVR; it does not prove a human answered. Status callbacks are best effort. A received signed callback only verifies callback delivery/signing; it does not prove the destination rang. Configure independent carrier fallback before moving production calls.

For DNI, add the registered numbers to a site's pool and follow [dynamic number insertion](/guides/dynamic-numbers/). Assignment, cooldown, late-call evidence, consent and fallback rules are shared across providers. Keep numbers dedicated to their pools. Verify a visit, number swap and real call on one site before expanding.

## CLI, MCP and public API

Credentials are configured through the workspace UI. Scoped API keys cannot save provider secrets.

```sh
visiconvert status
visiconvert numbers available --provider signalwire
visiconvert numbers add +12025550142 --provider signalwire \
  --forward-to +12025550143 --source "Yard signs" --campaign "Spring"
```

MCP exposes `list_signalwire_numbers`. Set `provider: "signalwire"` on `add_tracking_number`; writes still require `--allow-changes` and a `numbers:write` key. `get_status` returns both provider statuses and callback URLs. Agents must not ask users to paste credentials in chat or change production carrier routing without approval.

The v1 API provides `GET /api/v1/connections/signalwire` and `GET /api/v1/connections/signalwire/numbers` with `read` scope. Register numbers with `POST /api/v1/numbers`, `numbers:write`, and `provider: "signalwire"`. Omitted provider defaults to `twilio` for compatibility. Inventory returns the first page and `has_more`; registering an exact number performs a filtered ownership check. No SMS, recordings, transcription, purchases, porting, automatic routing changes or Google/Microsoft conversion delivery are included in this slice.

## Protocol and verification

Only form-encoded Compatibility callbacks are accepted. Signatures use the project's signing key, canonical public URL and sorted form fields (HMAC-SHA1/base64). `X-SignalWire-Signature` and Compatibility's `X-Twilio-Signature` are accepted; conflicting/duplicate signature headers and duplicate form fields are rejected. Account ID and registered number provider must match the workspace connection. Call IDs are provider-namespaced; receipts remain workspace-scoped. This endpoint does not accept SWML JSON webhook signatures.

Tests cover simulated provider verification, ownership, encryption separation, credential rotation, callback authorization, tenant isolation, retries, call ordering, and UI setup. A live carrier call is a separate release acceptance check; simulations cannot establish production carrier readiness.

References: [account retrieval](https://signalwire.com/docs/compatibility-api/rest/accounts/get-account), [number inventory](https://signalwire.com/docs/compatibility-api/rest/incoming-phone-numbers/list-incoming-phone-numbers), [webhook verification](https://signalwire.com/docs/compatibility-api/guides/webhooks), [cXML Dial](https://signalwire.com/docs/compatibility-api/cxml/reference/voice/dial).
