/* VisiConvert website capture — AGPL-3.0-only. No automatic form listeners. */
(() => {
  "use strict";
  const script = document.currentScript;
  const site = script?.dataset.site;
  if (!site || window.VisiConvert) return;
  const base = new URL(script.src).origin;
  const endpoint = `${base}/collect/sites/${encodeURIComponent(site)}`;
  const storageKey = `visiconvert:${site}:session:v1`;
  const campaignKeys = ["utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content", "gclid", "gbraid", "wbraid", "msclkid", "fbclid"];
  let consent = false, generation = 0, current = null, pending = null;
  const controllers = new Set();
  const storage = {
    get() { try { return JSON.parse(sessionStorage.getItem(storageKey)); } catch { return null; } },
    set(value) { try { sessionStorage.setItem(storageKey, JSON.stringify(value)); } catch { /* memory-only works too */ } },
    clear() { try { sessionStorage.removeItem(storageKey); } catch { /* storage may be blocked */ } },
  };
  function requireConsent(epoch) {
    if (!consent || epoch !== generation) throw new Error("VisiConvert tracking consent is required");
  }
  async function request(path, body, epoch) {
    requireConsent(epoch);
    const controller = new AbortController(); controllers.add(controller);
    try {
      const response = await fetch(endpoint + path, {
        method: body ? "POST" : "GET", mode: "cors", credentials: "omit", referrerPolicy: "no-referrer",
        headers: body ? { "Content-Type": "application/json" } : {},
        body: body ? JSON.stringify(body) : undefined, signal: controller.signal,
      });
      requireConsent(epoch);
      const result = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(result.error || `VisiConvert collection failed (${response.status})`);
      return result;
    } finally { controllers.delete(controller); }
  }
  function context() {
    const url = new URL(location.href);
    const campaign = new URLSearchParams();
    for (const key of campaignKeys) if (url.searchParams.has(key)) campaign.set(key, url.searchParams.get(key));
    url.search = campaign.toString(); url.hash = ""; url.username = ""; url.password = "";
    let referrer = "";
    try { referrer = new URL(document.referrer).origin; } catch { /* no referrer */ }
    return { id: crypto.randomUUID(), consent: "granted", landing_url: url.href, referrer };
  }
  async function session(epoch) {
    requireConsent(epoch);
    if (pending) return pending;
    const work = async () => {
      const saved = current || storage.get();
      current = saved && saved.started > Date.now() - 29 * 60 * 1000 && saved.started <= Date.now() && saved.payload?.id
        ? saved : { started: Date.now(), payload: context() };
      storage.set(current);
      // Reposting immutable context is idempotent and proves that this server received it.
      await request("/sessions", current.payload, epoch);
      return current.payload.id;
    };
    const job = work(); pending = job;
    try { return await job; } finally { if (pending === job) pending = null; }
  }
  window.VisiConvert = Object.freeze({
    async setConsent(granted) {
      if (granted !== true) {
        consent = false; generation++; current = null; pending = null; storage.clear();
        for (const controller of controllers) controller.abort();
        return { tracking: false };
      }
      consent = true;
      await session(generation);
      return { tracking: true };
    },
    async trackForm({ submissionId, fields = {} } = {}) {
      const epoch = generation; requireConsent(epoch);
      if (typeof submissionId !== "string" || !/^[a-zA-Z0-9_.:-]{1,80}$/.test(submissionId)) throw new Error("Use your form backend's stable submission ID (up to 80 characters)");
      // Never inspect the DOM or serialize a form. Only these explicitly supplied,
      // site-allowed fields can leave the browser, even if extra fields are passed.
      const config = await request("/config", null, epoch);
      const selected = {};
      for (const key of ["name", "email", "phone", "company"]) {
        if (config.allowed_fields.includes(key) && Object.hasOwn(fields, key)) {
          if (typeof fields[key] !== "string") throw new Error("Allowed form fields must be strings");
          selected[key] = fields[key];
        }
      }
      const sessionId = await session(epoch);
      return request("/forms", { submission_id: submissionId, session_id: sessionId, consent: "granted", fields: selected }, epoch);
    },
  });
  window.dispatchEvent(new CustomEvent("visiconvert:ready"));
})();
