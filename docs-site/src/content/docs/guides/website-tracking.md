---
title: Website tracking
description: Install the tracker on your site, connect it to your consent manager and record successful form submissions.
---

Website tracking records where a visit came from (UTM tags, ad click IDs, referrer) and attaches it to forms submitted in the same visit. It works the same way on hosted and self-hosted workspaces.

## 1. Add your site

In the web app, open **Sites** and add the exact origin, such as `https://www.example.com`. `www`, other subdomains and ports are separate origins; there are no wildcards. Pick which form fields you allow: `name`, `email`, `phone` and `company` are supported.

## 2. Install the script

Copy the script tag from **Sites** into your site's HTML. It loads from your VisiConvert server, which must be on HTTPS for a public site. If your site has a Content Security Policy, allow your VisiConvert origin in `script-src` and `connect-src`.

The site ID in the tag is public. Never put an API key in website code.

## 3. Connect your consent manager

The script does nothing until you say tracking is allowed: no requests and no browser storage. Call this from your consent manager when consent is granted, including when restoring a saved choice:

```js
await window.VisiConvert.setConsent(true);
```

Call `setConsent(false)` when consent is withdrawn.

## 4. Report successful forms

VisiConvert does not watch your forms. You call it after your backend confirms a submission succeeded, with that submission's ID:

```js
try {
  await window.VisiConvert.trackForm({
    submissionId: result.submissionId,
    fields: { name: values.name, email: values.email, phone: values.phone },
  });
} catch (error) {
  // Log it, but never fail the customer's form because tracking failed.
}
```

`submissionId` must be unique per real submission (1–80 letters, digits, `_`, `-`, `.` or `:`). Sending the same ID again with the same values is safe and creates one lead. A thank-you page visit on its own is not enough.

Do not send passwords, payment details or free-text messages.

## What gets recorded

- Landing page path (without query string), UTM tags, and `gclid`, `gbraid`, `wbraid`, `msclkid` or `fbclid` if present.
- The referring site's origin only.
- Explicit `utm_source` takes priority. Without it, Google or Microsoft click IDs supply a source; `fbclid` identifies Facebook without assuming paid traffic.
- Conflicting click IDs stay **Unknown / conflicting click IDs**. Conflicting UTMs and click IDs are highlighted in the lead’s evidence, with the original inputs preserved.
- Otherwise recognized search/social referrers supply a source; other referring sites retain their origin. Missing evidence stays **Direct / unknown**.

These are browser-reported hints, not proof of an ad click. The same classification applies to website forms and DNI calls. Existing leads keep their original source. Dedicated campaign numbers and the UTM-only URL preview are unchanged.

A visit is a 30-minute session in one browser tab. There is no cross-device or multi-touch attribution.

## Check it works

Submit a real form on your site, then refresh **Sites**: it shows sessions and forms received and when the last one arrived. If nothing arrives, check that the site is enabled, the origin matches exactly, consent was granted, and your CSP or an ad blocker isn't stopping the request.

## Limits

Each site accepts up to 120 new sessions and forms per minute. Browser-reported forms can be spam, so qualify them in the inbox like any other lead.

Full technical reference: [WEBSITE_TRACKING.md](https://github.com/VisiOffice/visiconvert/blob/main/docs/WEBSITE_TRACKING.md).
