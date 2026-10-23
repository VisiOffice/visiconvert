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
      requireConsent(epoch);
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
  // Only explicit, phone-only markers are changed. Original markup stays intact.
  const dniEnabled = script.dataset.dni === "true";
  const originals = new Map();
  let lease = null, refreshTimer = null, expiryTimer = null, numberJob = null;
  let numberGeneration = 0;
  const phone = value => { if (!/^\s*\+?[0-9\s().-]+\s*$/.test(String(value || ""))) return ""; const digits = String(value || "").replace(/[^0-9]/g, ""); return digits.length === 10 ? `+1${digits}` : digits.length === 11 && digits[0] === "1" ? `+${digits}` : ""; };
  function restoreNumbers() {
    lease = null;
    clearTimeout(expiryTimer);
    for (const [node, original] of originals) {
      if (original.aria !== null && node.getAttribute("aria-label") === original.replacement) node.setAttribute("aria-label", original.aria);
      if (node.textContent === original.replacement) node.textContent = original.text;
      if (node.getAttribute("href") === original.link) {
        if (original.href === null) node.removeAttribute("href"); else node.setAttribute("href", original.href);
      }
    }
    originals.clear();
  }
  function stopNumbers() {
    if (!dniEnabled) return;
    numberGeneration++; numberJob = null;
    clearTimeout(refreshTimer); restoreNumbers();
  }
  function applyNumbers() {
    if (!lease || !consent || Date.now() >= lease.until) { if (dniEnabled) restoreNumbers(); return; }
    for (const node of document.querySelectorAll("[data-visiconvert-phone]")) {
      if (originals.has(node) || node.children.length || phone(node.textContent) !== lease.fallback_phone) continue;
      const href = node.getAttribute("href");
      if (href !== null && (!href.startsWith("tel:") || phone(href.slice(4)) !== lease.fallback_phone)) continue;
      const replacement = lease.phone_number;
      const link = href === null ? null : `tel:${replacement}`;
      const aria = node.getAttribute("aria-label");
      if (aria !== null && phone(aria) !== lease.fallback_phone) continue;
      originals.set(node, { text: node.textContent, href, replacement, link, aria });
      if (aria !== null) node.setAttribute("aria-label", replacement);
      node.textContent = replacement;
      if (link !== null) node.setAttribute("href", link);
    }
  }
  async function refreshNumbers() {
    if (!dniEnabled || !consent || document.hidden) return { status: "off" };
    if (numberJob) return numberJob;
    const epoch = generation, attempt = numberGeneration;
    const job = (async () => {
      // Fail closed on a stalled request, even if fetch never rejects.
      let timeout;
      try {
        const result = await Promise.race([
          (async () => { const id = await session(epoch); return request("/numbers", { session_id: id, consent: "granted" }, epoch); })(),
          new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("Number request timed out")), 8000); }),
        ]);
        requireConsent(epoch);
        if (attempt !== numberGeneration) return { status: "off" };
        restoreNumbers();
        const until = Date.parse(result.expires_at);
        if (result.status === "assigned" && /^\+1[0-9]{10}$/.test(result.phone_number) && /^\+1[0-9]{10}$/.test(result.fallback_phone) && until > Date.now() && until <= Date.now() + 31 * 60 * 1000) {
          lease = { ...result, until };
          applyNumbers();
          expiryTimer = setTimeout(restoreNumbers, until - Date.now());
        }
        return result;
      } catch {
        if (attempt === numberGeneration) restoreNumbers();
        return { status: "unavailable" };
      } finally {
        clearTimeout(timeout);
        if (consent && attempt === numberGeneration) {
          clearTimeout(refreshTimer);
          refreshTimer = setTimeout(() => { void refreshNumbers(); }, 60000);
        }
      }
    })();
    numberJob = job;
    try { return await job; } finally { if (numberJob === job) numberJob = null; }
  }
  if (dniEnabled) {
    new MutationObserver(() => { if (lease) applyNumbers(); }).observe(document.documentElement, { childList: true, subtree: true });
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) stopNumbers(); else if (consent) void refreshNumbers();
    });
    window.addEventListener("pagehide", stopNumbers);
    window.addEventListener("pageshow", () => { if (consent) void refreshNumbers(); });
  }
  window.VisiConvert = Object.freeze({
    async setConsent(granted) {
      if (granted !== true) {
        stopNumbers();
        consent = false; generation++; current = null; pending = null; storage.clear();
        for (const controller of controllers) controller.abort();
        return { tracking: false };
      }
      consent = true;
      try { await session(generation); } catch (error) { stopNumbers(); throw error; }
      if (dniEnabled) await refreshNumbers();
      return { tracking: true };
    },
    refreshNumbers,
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
