import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';
const source = readFileSync(new URL('../../crates/convert-server/assets/tracker.js', import.meta.url), 'utf8');
function fixture({ blockedStorage = false, failForm = false, jsonGate = null } = {}) {
  const requests = [], saved = new Map();
  const window = { dispatchEvent() {} };
  const ctx = vm.createContext({ window, document: { currentScript: { src: 'https://engine.example/tracker.js', dataset: { site: 'site-123' } }, referrer: 'https://search.example/private?secret=value' }, location: { href: 'https://business.example/quote?utm_source=google&gclid=abc&password=secret#fragment' }, URL, URLSearchParams, crypto: webcrypto, AbortController, CustomEvent: class {}, Date, sessionStorage: {
    getItem(k) { if (blockedStorage) throw Error('blocked'); return saved.get(k); }, setItem(k,v) { if (blockedStorage) throw Error('blocked'); saved.set(k,v); }, removeItem(k) { saved.delete(k); }
  }, fetch: async (url, options) => {
    const body = options.body ? JSON.parse(options.body) : null;
    requests.push({ url, options, body });
    if (failForm && url.endsWith('/forms')) { failForm = false; throw Error('network failed'); }
    return { ok: true, json: async () => { if (jsonGate) await jsonGate; return url.endsWith('/config') ? { allowed_fields: ['name','email'] } : { accepted: true }; } };
  } });
  vm.runInContext(source,ctx);
  return { api: window.VisiConvert, requests, saved };
}
test('no capture before consent, allowlist before network, sanitized source, withdrawal clears state',async () => {
  const {api, requests, saved} = fixture();
  assert.equal(requests.length,0);
  await assert.rejects(api.trackForm({submissionId:'a',fields:{}}),/consent/);
  assert.equal(requests.length,0);
  await api.setConsent(true);
  assert.equal(requests.length,1);
  assert.equal(requests[0].body.landing_url,'https://business.example/quote?utm_source=google&gclid=abc');
  assert.equal(requests[0].body.referrer,'https://search.example');
  await api.trackForm({submissionId:'saved-1',fields:{name:'Jane',email:'j@example.com',phone:'excluded',password:'secret',notes:'private'}});
  const form = requests.at(-1);
  assert.deepEqual(form.body.fields,{name:'Jane',email:'j@example.com'});
  assert.equal(form.options.credentials,'omit'); assert.equal(form.options.referrerPolicy,'no-referrer');
  assert(!JSON.stringify(requests).includes('private'));
  assert(!JSON.stringify(requests).includes('secret'));
  assert(![...saved.values()].join().includes('j@example.com'));
  await api.setConsent(false); assert.equal(saved.size,0);
  const count = requests.length; await assert.rejects(api.trackForm({submissionId:'b'}),/consent/); assert.equal(requests.length,count);
});
test('failed form retries preserve session and payload without persistent contact data',async () => {
  const {api,requests} = fixture({failForm:true,blockedStorage:true});
  await api.setConsent(true);
  const input = {submissionId:'backend-1',fields:{name:'Jane',email:'j@example.com'}};
  await assert.rejects(api.trackForm(input),/network failed/);
  await api.trackForm(input);
  const forms = requests.filter(r=>r.url.endsWith('/forms'));
  assert.equal(forms.length,2); assert.deepEqual(forms[0].body,forms[1].body);
});
test('invalid submission ID never creates a form request',async () => {
  const {api,requests} = fixture(); await api.setConsent(true);
  await assert.rejects(api.trackForm({submissionId:'contains spaces'}),/stable submission ID/);
  assert.equal(requests.length,1);
});

test('consent withdrawal while a response body is pending cannot complete capture', async () => {
  let release;
  const jsonGate = new Promise(resolve => { release = resolve; });
  const {api, requests, saved} = fixture({jsonGate});
  const pending = api.setConsent(true);
  await Promise.resolve();
  assert.equal(requests.length,1);
  await api.setConsent(false);
  release();
  await assert.rejects(pending,/consent/);
  assert.equal(saved.size,0);
});
