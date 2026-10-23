# Website tracking and successful forms

Website capture runs in the shared Rust service and Vite app. It works without Rails, billing, VisiComms, or a VisiConvert account on self-hosted installations. Hosted workspaces use the same routes and implementation.

## Setup

1. Open **Sites**, add a name and the exact website origin (for example `https://www.example.com`), and select allowed form fields. HTTPS is required except on localhost for development. `www`, other subdomains, protocols, and ports are separate origins. There are no wildcard domains. Add a separate site for each origin (up to 50 per workspace).
2. Copy the generated script tag into your site's HTML. Public HTTPS websites require an HTTPS VisiConvert endpoint (`PUBLIC_URL`); a localhost installation is only suitable for local tests. Allow your VisiConvert service origin in the website's `script-src` and `connect-src` CSP directives. Install one site ID per page. The ID is public; never put the workspace API key in a website.
3. Integrate your consent manager. Loading the script defines `window.VisiConvert` and dispatches `visiconvert:ready`; it sends no collection request and accesses no browser storage until `setConsent(true)` is called. Call after the script is ready, including when restoring an existing consent choice. Catch rejected promises.
4. Integrate your form backend's **confirmed successful submission** response. VisiConvert does not listen for submit or click events, inspect forms, or assume a validation pass means success. AJAX/SPA forms can call the hook directly. Full navigation forms must carry the backend's stable submission ID to the success page. A generic thank-you-page visit alone is insufficient.

```js
// Called by your consent manager after it grants tracking consent.
await window.VisiConvert.setConsent(true);

// In your existing form success handler, after your backend confirms success:
try {
  await window.VisiConvert.trackForm({
    submissionId: result.submissionId,
    fields: { name: values.name, email: values.email, phone: values.phone }
  });
} catch (error) {
  // Report tracking failure separately; never fail the customer's form submission.
  // A retry must use the same backend submission ID and the same allowed values.
}

// In your consent withdrawal handler:
await window.VisiConvert.setConsent(false);
```

`submissionId` is required: 1–80 letters, digits, `_`, `-`, `.`, or `:`. The backend must issue a new ID for each actual success. Duplicate callbacks with identical session/allowed-field payloads create one lead, including concurrent requests. Conflicting payloads return 409, so reusing an ID cannot overwrite a lead or its source. Retry a failed request promptly in the same session with the original values; no automatic background form retry queue or persistent contact data is used. Delivery is best effort, so navigation, blockers, offline browsers, or revoked consent can prevent capture. Handle the promise before navigating if collection is wanted.

Only `name`, `email`, `phone`, and `company` are supported, and only those enabled on the site are sent. Extra input fields are discarded by the browser; the server rejects direct requests containing disallowed fields. String values are capped at 200 bytes and cannot contain control characters. An empty allowlist supports anonymous inquiries. Do not put passwords, payment data, sensitive information, or free-form messages into these fields. The tracker never reads field values from the DOM or stores them in browser storage.

## Attribution and diagnostics

A session captures the entry context after consent, with a random ID in tab-scoped session storage. Storage-blocked browsers work in memory. Sessions are fixed 30-minute windows; the browser starts a fresh window after 29 minutes to leave time for delivery. Repeated collection of the same session is idempotent. Refreshes/SPA navigation within that window retain its entry context; this release does not record pageviews or implement cross-tab/device visitor identity, first-touch history, or last-non-direct attribution. Optional [dynamic number insertion](DYNAMIC_NUMBERS.md) uses the same sessions.

Captured evidence:

- Landing origin/path, without query or fragment. Ensure sensitive information is not embedded in URL paths.
- `utm_source`, `utm_medium`, `utm_campaign`, `utm_term`, `utm_content`.
- `gclid`, `gbraid`, `wbraid`, `msclkid`, `fbclid` when present and tracking consent is granted. Each value is capped at 200 bytes.
- Referrer origin only, without path/query/fragment.
- Reported consent `granted`, attribution model `session-entry-v1`, and confidence `browser-reported`.

Lead source uses explicit `utm_source`, otherwise an external referrer origin, otherwise **Direct / unknown**. Missing evidence is not reconstructed. Click IDs do not imply verified ad delivery. The lead drawer shows the immutable source evidence and captured fields. Existing manual forms and call leads keep their current behavior.

Sites shows received session/form counts and last-received timestamps. Refresh diagnostics after completing a real successful form. If no session arrives, check enabled status, exact origin, consent, CSP, blockers, and the browser network panel. Pausing a site rejects new collection immediately without deleting historical leads. Editing fields/origin affects future accepted requests; it does not erase previous evidence.

## Public collection and limits

`GET /tracker.js` serves the standalone script embedded in the server binary. Public endpoints are `/collect/sites/{id}/config` (GET), `/sessions` (POST), `/forms` (POST), and optional `/numbers` (POST), plus OPTIONS for browser preflight. Collection requires the exact allowed Origin and returns CORS headers only for that origin. It never uses cookies, returns private lead IDs, or grants private API access. Collection requires a granted consent value, an active site, valid context, and a recent session belonging to that site. Owner-authenticated `/api/sites` manages sites. `/api/leads/{id}/website-evidence` is workspace scoped.

Public IDs and Origin headers are **not authentication**: a non-browser client can forge them. Browser-reported forms can be spam and should be qualified in the inbox; they are not a trusted payment or booking event. Limits are 8 KiB per collection request and 120 new sessions/forms/number-allocation attempts per site per minute, enforced atomically in PostgreSQL. Identical retries do not consume this quota. This is a basic shared limit, not bot detection; busy sites or an attacker can exhaust it. Private APIs and the rest of a workspace remain separate. Production network-level abuse controls are still advisable when scaling collection.

Unconverted sessions with no number assignments are eligible for cleanup. Cleanup is opportunistic: each new session removes up to 100 sessions older than 30 days for that site. Dormant sites are not automatically purged. Converted evidence follows existing lead retention and remains in backups; this slice does not introduce an erasure or retention scheduler. Consent withdrawal clears the browser's session and aborts pending client requests; it cannot undo an already accepted request or erase server history. Integration owners are responsible for mapping their consent policy to this explicit tracking gate.

## Verification

- `node --test web/tests/tracker.test.mjs`: consent, network field filtering, privacy, storage failure, retries.
- `cargo test -p convert-server site_tests:: -- --ignored --test-threads=1` with a **disposable** `VISICONVERT_TEST_DATABASE_URL`: real PostgreSQL, hosted/self-hosted access, isolation, duplicate races, expiry, rate limits, CORS and pause.
- `pnpm --dir web test:browser`: isolated database plus actual cross-origin website/script and production app; validates setup, no capture on failed validation/submit attempts, confirmed success, one lead after retry, diagnostics, evidence and responsive UI.

Source and migration are included in the self-host package. Back up the database before upgrade; migration 0005 is additive. No Rails migration or environment secret is required.
