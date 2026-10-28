# Dynamic number insertion (DNI)

DNI runs in the shared Rust service, website tracker, and Vite app. Both hosted and self-hosted workspaces have the same feature. Rails, billing, and VisiComms are not required. It uses existing registered US/Canada (+1) Twilio or SignalWire numbers; it does not buy numbers or configure carrier callbacks automatically.

## Configure a website pool

1. Connect Twilio or SignalWire and register real tracking numbers under **Tracking numbers**. Configure each number's voice and status callbacks as shown there. Forward every number in a pool to the same business destination (which may be your call center's number).
2. In **Sites**, select the exact website origin. Under **Dynamic number insertion**, enter the **published website number** to replace (`fallback_phone`), then the **forwarding destination** (`destination_phone`). These can differ: a local published number can be replaced by a local pool while every pool number rings a central call center. Select 1–50 registered numbers with that destination. Keep the published number outside every pool; if it is a registered static number, it must belong to this workspace and forward to the same destination. Confirm these numbers are exclusively for this website, enable swapping, and save.
3. Add `data-dni="true"` to the site's tracker script. Mark phone-only elements explicitly:

```html
<script defer src="https://YOUR-VISICONVERT-HOST/tracker.js"
  data-site="YOUR-PUBLIC-SITE-ID" data-dni="true"
  referrerpolicy="no-referrer"></script>
<a data-visiconvert-phone href="tel:+12025550106">(202) 555-0106</a>
<span data-visiconvert-phone>202-555-0106</span>
```

The displayed number and `tel:` destination must match the configured fallback. Marked elements must have no child elements, extra words, extensions, or different destination. Put icons outside the marked element. A phone-only `aria-label`, when present, must also match and is updated/restored. Unmarked content is untouched. Dynamically inserted marked elements are handled by the tracker; it does not scan arbitrary page text or forms.

4. Use the existing consent integration: after `visiconvert:ready`, call `await window.VisiConvert.setConsent(true)` only after consent is granted. Call `setConsent(false)` on withdrawal. Catch tracking errors separately from customer interactions. No collection request or browser storage access occurs before consent. Without the DNI script attribute, existing form-only installations behave as before.
5. Open a campaign URL, grant consent, verify the displayed number and tap-to-call destination, then make a real call. Inspect the call's **Call source evidence** in its inquiry. Verify the business destination actually receives the call. Automated tests use synthetic signed callbacks; real carrier delivery still requires this acceptance check.

## Assignment and fallback behavior

Assignments end 30 minutes after the session started, followed by another 30 minutes before reuse. Retry of the same session returns its existing assignment; refreshing never extends the window. The tracker starts a new session after 29 minutes. One number can have only one reservation at a time. Pool allocation is serialized in PostgreSQL, including simultaneous visitors and configuration changes.

Numbers remain dedicated to their original site after removal or disabling. This preserves the meaning of delayed calls; moving numbers between sites is not supported in this slice. Pausing collection, changing the origin, disabling a pool, changing the published number or forwarding destination, or removing a number revokes its current assignment but does not free its reservation early. Withdrawal in the browser likewise does not release a number someone may have saved.

The published website number remains visible if consent is absent, the pool is exhausted/disabled, or collection fails. Visible pages refresh assignments every minute; a local expiry timer restores the original text and link. Failed requests restore originals, and stalled refreshes fall back after eight seconds. Hidden pages restore immediately and check again when shown. Browser suspension or timers can delay execution; the next visibility/page event rechecks. SPA applications that replace marked content should use new elements, or call `await window.VisiConvert.refreshNumbers()` after updating it.

Pool diagnostics show selected numbers, reservations/cooldowns, an exhaustion-response count (including repeated exhausted polls), and the latest 50 assignments. Polling diagnostics does not change configuration. Saving uses optimistic versions; refresh before retrying a conflicting edit.

## Google Business Profile and other static sources

Use **Setup → Google Business Profile** for one dedicated static number per listing. Register it with source `Google Business Profile`, the location as campaign, and the business destination. Configure carrier callbacks and verify a real call before updating the listing. In Google, set the tracking number as the primary phone and keep the business number as an additional phone. VisiConvert does not edit Google listings or carrier settings.

Keep these numbers outside DNI pools. A listing call receives the number’s static source/campaign without inventing a website session or click ID. Ads using that listing’s phone number may reach it too; the label does not establish organic traffic. Website clicks from the listing can be tagged with UTMs and use the website’s separate DNI pool.

See [CallRail’s listing setup](https://support.callrail.com/hc/en-us/articles/8435634700685-Google-My-Business-multiple-location-setup) and [Google’s phone field documentation](https://support.google.com/business/answer/16750496?hl=en).

## Evidence and limits

On the first signed **voice** callback, a call to a pooled number receives the current assignment's immutable session-entry evidence, source, and campaign. The forwarding destination remains the registered business line. Calls do not wait for a carrier API request or number allocation.

The match is explicitly **inferred from the observed callback window**, not verified caller identity. The callback has no trusted call-start timestamp in this implementation. Saved numbers, shared devices, and delayed webhooks may belong to an earlier visitor, even after the reuse delay. Evidence records this warning and whether the number was previously assigned. A status/dial callback arriving first, a disabled pool, or no active assignment produces **Unknown / pooled call**. Later callbacks and retries never rewrite the original evidence. Static numbers outside pools retain their existing attribution.

Calls and forms remain separate interactions. DNI does not automatically combine identities/inquiries; explicit grouping preserves every interaction and booking. Inquiry reporting uses its chosen primary interaction as before. This is session-entry attribution, not cross-device, multi-touch, or proof that advertising caused a booking. SMS, recordings, number purchase, adaptive pool sizing, bot detection, and geographic number formats beyond +1 remain future work.

Public `/collect/sites/{id}/numbers` accepts `{session_id, consent:"granted"}` with the exact configured Origin and a recent accepted session. It returns only assignment status, phone/fallback, and expiry; no private history or lead IDs. Site IDs and Origin headers are public and forgeable by non-browser clients. An attacker can exhaust a pool. The existing atomic 120/minute site quota covers new sessions/forms and new allocation attempts (including exhaustion); idempotent accepted-session/assignment retries do not consume it. The 8 KiB request cap also applies.

Owner-authenticated `GET/PUT /api/sites/{id}/number-pool` manages configuration and diagnostics. Workspace-scoped `GET /api/leads/{id}/dni-evidence` exposes evidence. Assignment history and referenced sessions are retained for audit with existing database/backups; they are excluded from unconverted-session cleanup. No scheduled retention/deletion policy is introduced here.

## Upgrade and verification

Back up before upgrading. Additive migration **0007_dynamic_numbers.sql** leaves existing sites/numbers unchanged until configured. Migration **0013_dni_destination.sql** backfills existing destinations from `fallback_phone`, preserving pool versions, enabled state, numbers, and reservations. New clients should send `destination_phone` explicitly. Older clients may omit it: updates preserve the saved destination; new pools default it to `fallback_phone`. The public tracker still receives only the published fallback, assigned number, and expiry. No new environment secret or Rails migration is required. The self-host bundle includes migration, UI, tracker, and these instructions.

- `cargo test -p convert-store --test dni -- --ignored --test-threads=1` against a disposable `VISICONVERT_TEST_DATABASE_URL`: simultaneous allocation, retry stability, isolation, cooldown, reuse, immutable call evidence, out-of-order callbacks, exact-cent booking totals, pause/re-enable, and origin changes.
- `cargo test -p convert-server site_tests:: -- --ignored --test-threads=1`: authenticated management, revoked access, origin, consent, and session checks.
- `pnpm --dir web test:browser`: production frontend and real database/API, pool setup, opted-in text/link swaps, dynamically added elements, exhaustion, network failure, local expiry, consent withdrawal, pause, and mobile layout. Network/expiry failure cases are deliberately injected; normal allocation uses the real API.
