---
title: Dynamic number insertion
description: Show each website visitor their own tracking number so calls are matched to the visit that produced them.
---

Dynamic number insertion (DNI) swaps the phone number on your website for one from a pool of tracking numbers. A call to that number is matched to the visit, so it gets the visit's source, campaign and click IDs.

It needs [website tracking](/guides/website-tracking/) installed first.

## 1. Prepare the numbers

Register the pool numbers under **Tracking numbers** and set their carrier webhooks, as in [Connect Twilio](/start/connect-twilio/) or [Connect SignalWire](/start/connect-signalwire/). Every number in a pool must forward to the same business line. Use them only for this website.

How many you need: each number is held for 30 minutes per visitor, then rests for 30 minutes, so each number serves about one new visitor an hour. When the pool runs out, visitors see your normal number.

## 2. Create the pool

From **Setup**, choose **Set up a website pool**, then open the site under **Sites → Dynamic number insertion**:

1. **Published website number:** the number your site already displays. It stays visible when tracking is unavailable.
2. **Forwarding destination:** the business or call center line that should ring. This can differ from the published number.
3. **Dedicated tracking numbers:** select 1–50 registered numbers that all forward to that destination. Keep the published number and any Google Business Profile numbers outside this pool.
4. Review the configuration, confirm exclusive use, and enable swapping.

For example, an LA website can display its local number, swap it for a visitor’s LA pool number, and send the call to your central call center. Phoenix uses its own published number and dedicated pool. Confirm the published numbers already reach your business, since they remain the fallback. Saving a pool does not change any carrier route.

## 3. Mark the numbers on your site

Add `data-dni="true"` to the tracker script, and `data-visiconvert-phone` to each element that shows your phone number:

```html
<script defer src="https://YOUR-VISICONVERT-HOST/tracker.js"
  data-site="YOUR-PUBLIC-SITE-ID" data-dni="true"
  referrerpolicy="no-referrer"></script>

<a data-visiconvert-phone href="tel:+12025550106">(202) 555-0106</a>
```

The marked element must contain only the phone number, matching your fallback, with no icons or other words inside it. Unmarked text is never changed.

If your app replaces marked elements after load, call `await window.VisiConvert.refreshNumbers()` afterwards.

## 4. Test it

Open your site with a campaign link, accept tracking consent, check the number changed, then call it. Open the call in the inbox and check **Call source evidence**, and confirm your business line rang.

## When visitors see your normal number

- They haven't given consent.
- The pool is used up or switched off.
- A request failed or took more than eight seconds.
- The assignment expired.

## How sure the match is

The match is inferred from timing, not from who is calling. Someone who saved the number, or calls back days later, can be matched to an earlier visitor. The evidence on each call says this. Calls that can't be matched show as **Unknown / pooled call**.

## Google Business Profile calls

Choose **Setup → Set up a Business Profile number** for calls directly from a Google listing. Use one dedicated static number per listing, with source **Google Business Profile** and the location as its campaign. Configure carrier callbacks and verify a call reaches the business before setting it as the primary phone in Google. Keep your business number as an additional phone. VisiConvert does not edit the listing for you.

This path needs no website pool or tracker. Keep the number outside all DNI pools. Calls get listing-level attribution without a visitor click ID. Ads using the listing’s number may reach it too, so this source does not mean every call was organic. Visitors who click through to your website can use the website’s separate DNI pool; tag the listing’s website link with UTMs if you want that source captured.

[CallRail’s documented listing setup](https://support.callrail.com/hc/en-us/articles/8435634700685-Google-My-Business-multiple-location-setup) · [Google’s phone fields](https://support.google.com/business/answer/16750496?hl=en)

Full technical reference: [DYNAMIC_NUMBERS.md](https://github.com/VisiOffice/visiconvert/blob/main/docs/DYNAMIC_NUMBERS.md).
