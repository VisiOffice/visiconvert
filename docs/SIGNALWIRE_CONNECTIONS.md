# SignalWire connections

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

For DNI, add the registered numbers to a site's pool and follow [DYNAMIC_NUMBERS.md](DYNAMIC_NUMBERS.md). Assignment, cooldown, late-call evidence, consent and fallback rules are shared across providers. Keep numbers dedicated to their pools. Verify a visit, number swap and real call on one site before expanding.

## CLI, MCP and public API

Credentials are configured through the workspace UI. Scoped API keys cannot save provider secrets.

```sh
visiconvert status
visiconvert numbers available --provider signalwire
visiconvert numbers add +12025550142 --provider signalwire \
  --forward-to +12025550143 --source "Yard signs" --campaign "Spring"
```

MCP exposes `list_signalwire_numbers`. Set `provider: "signalwire"` on `add_tracking_number`; writes still require `--allow-changes` and a `numbers:write` key. `get_status` returns both provider statuses and callback URLs. Agents must not ask users to paste credentials in chat or change production carrier routing without approval.

The v1 API provides `GET /api/v1/connections/signalwire` and `GET /api/v1/connections/signalwire/numbers` with `read` scope. Register numbers with `POST /api/v1/numbers`, `numbers:write`, and `provider: "signalwire"`. Omitted provider defaults to `twilio` for compatibility. Inventory returns the first page and `has_more`; registering an exact number performs a filtered ownership check. No SMS, recordings, transcription, porting or Google/Microsoft conversion delivery are included in this slice.

## Buy and release numbers

Owners can buy local numbers into the connected project instead of registering existing ones:

- `POST /api/v1/numbers/search` with `{"area_code":"213"}` lists available local voice numbers. Nothing is reserved.
- `POST /api/v1/numbers/purchase` with a client-chosen `request_id` and the usual number fields buys the number, sets its voice and status webhooks to this workspace's URLs and registers it. Retrying the same `request_id` never buys twice; if registration fails, cleanup is persisted and retried. A timeout may mean the carrier already bought the number: keep the original request ID and inspect `GET /api/v1/numbers/purchases/{id}`. The worker reconciles inventory using the request-specific carrier label and never repeats the purchase POST. An unresolved request continues to reserve capacity. Purchases are billable at your carrier and capped per workspace by `VISICONVERT_PURCHASE_LIMIT` (default 25).
- `POST /api/v1/numbers/{id}/release` starts a hold of `VISICONVERT_RELEASE_HOLD_DAYS` (default 45). The number keeps forwarding calls but is disabled in its website pool; `.../restore` cancels the hold only before carrier deletion starts. Once deletion may have started, restoration is rejected even if the carrier response is uncertain. Website fallback references must be replaced before release; a held number cannot become a fallback. After the hold, the engine releases the number at the carrier. Its call history is kept.

API keys need the separate `numbers:purchase` permission. Hosted workspaces can instead let VisiConvert Cloud provision a dedicated SignalWire project; that connection is marked managed and cannot be edited or disconnected by the owner. New managed purchases are currently blocked by a separate Rails eligibility check until usage billing is implemented. BYOC purchases remain available and are charged by the customer’s carrier.

## Protocol and verification

Only form-encoded Compatibility callbacks are accepted. Signatures use the project's signing key, canonical public URL and sorted form fields (HMAC-SHA1/base64). `X-SignalWire-Signature` and Compatibility's `X-Twilio-Signature` are accepted; conflicting/duplicate signature headers and duplicate form fields are rejected. Account ID and registered number provider must match the workspace connection. Call IDs are provider-namespaced; receipts remain workspace-scoped. This endpoint does not accept SWML JSON webhook signatures.

Tests cover simulated provider verification, ownership, encryption separation, credential rotation, callback authorization, tenant isolation, retries, call ordering, and UI setup. A live carrier call is a separate release acceptance check; simulations cannot establish production carrier readiness.

References: [account retrieval](https://signalwire.com/docs/compatibility-api/rest/accounts/get-account), [number inventory](https://signalwire.com/docs/compatibility-api/rest/incoming-phone-numbers/list-incoming-phone-numbers), [webhook verification](https://signalwire.com/docs/compatibility-api/guides/webhooks), [cXML Dial](https://signalwire.com/docs/compatibility-api/cxml/reference/voice/dial).

## Purchase recovery and upgrades

Purchase status is workspace-scoped and returns no provider credentials. States are
`pending` (carrier outcome or registration unresolved), `registered`, `cleanup`
(carrier deletion must be retried), `failed` (definitively rejected or cleaned up),
and `review` (legacy request lacking recovery evidence). Background recovery runs
once per minute. Missing inventory stays pending: it is not proof that a timed-out
purchase failed. The original payload, project and carrier identifier are retained;
changing the forwarding destination under an existing request ID is rejected.

Unresolved purchases block disconnecting/replacing their project, as well as manual
registration of the same phone. Token rotation within that project is still allowed.
Do not manually delete receipts or reset them to failed without reconciling carrier
ownership and billing. Migration 0015 puts pre-existing pending/failed attempts into
`review`, because the previous implementation did not retain enough carrier evidence.
Check those against SignalWire before clearing them. This release does not invent
proof for purchases made by an older engine.

Deploy Rails and the engine together: the gateway must expose the service-authenticated
`/internal/managed-number-access` check to the engine. Stop all old engine processes
before applying migration 0015 and starting this release; old workers do not implement
the new recovery protocol. Ordinary subscription access does not enable managed
carrier spending. Usage metering, credit reservations and invoicing remain separate
Rails work; no usage prices or included credit are promised by this release.
