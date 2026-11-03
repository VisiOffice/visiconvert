import assert from 'node:assert/strict';
import path from 'node:path';

// UI contract test. The Rust integration suite tests delivery, signing and retries for real.
export async function verifyWebhooks({ page, api, artifactDir }) {
  // This harness runs without an encryption key, so the real API must refuse clearly.
  const refused = await api('/webhooks', { url: 'https://hooks.example.com/x', event_types: ['interaction.created'] });
  assert.equal(refused.status, 503);
  assert.match(refused.body.error, /VISICONVERT_ENCRYPTION_KEY/);

  const endpoint = { id: '00000000-0000-4000-8000-0000000000a1', url: 'https://crm.example.com/hooks/visiconvert', description: 'CRM sync', event_types: [], include_contact: true, state: 'enabled', disabled_reason: null, failing_since: null, secret_rotating_until: null, version: 1, created_at: new Date().toISOString() };
  const state = { endpoints: [], deliveries: [], requests: [] };
  const json = (route, body, status = 200) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  await page.route('**/api/webhook**', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const method = request.method();
    const body = request.postData() ? request.postDataJSON() : null;
    state.requests.push(`${method} ${url.pathname}`);
    if (url.pathname === '/api/webhooks' && method === 'GET') return json(route, { endpoints: state.endpoints, event_types: [], history: [{ id: 1, endpoint_id: endpoint.id, kind: 'created', actor: 'workspace operator', created_at: endpoint.created_at }] });
    if (url.pathname === '/api/webhooks' && method === 'POST') {
      assert.equal(body.url, endpoint.url); assert.equal(body.description, 'CRM sync'); assert.equal(body.include_contact, false);
      assert.deepEqual(body.event_types, ['interaction.created', 'call.completed', 'inquiry.reviewed', 'booking.recorded', 'booking.revised', 'booking.canceled', 'conversion.delivered']);
      Object.assign(endpoint, { event_types: body.event_types, include_contact: body.include_contact });
      state.endpoints = [endpoint];
      return json(route, { ...endpoint, secret: 'whsec_fixturesecretnotreal000000000000000000=' });
    }
    if (url.pathname.endsWith('/test')) {
      state.deliveries = [{ id: 'd1', event_id: 'e1', type: 'webhook.test', state: 'failed', attempts: 1, last_status: 400, last_error: 'The endpoint returned 400', last_attempt_at: new Date().toISOString(), next_attempt_at: null, created_at: new Date().toISOString() }];
      return json(route, { delivery_id: 'd1' });
    }
    if (url.pathname.endsWith('/deliveries')) return json(route, { data: state.deliveries, next_cursor: null });
    if (url.pathname.endsWith('/redeliver')) { state.deliveries[0] = { ...state.deliveries[0], state: 'succeeded', last_status: 204, last_error: null, attempts: 2 }; return json(route, { ok: true }); }
    if (url.pathname.endsWith('/rotate-secret')) { assert.equal(body.version, endpoint.version); endpoint.version += 1; endpoint.secret_rotating_until = new Date(Date.now() + 86400000).toISOString(); return json(route, { ...endpoint, secret: 'whsec_rotatedsecretnotreal0000000000000000=' }); }
    if (method === 'PATCH') { assert.equal(body.version, endpoint.version); assert.equal(body.url, endpoint.url); Object.assign(endpoint, { state: body.state ?? endpoint.state, version: endpoint.version + 1 }); return json(route, endpoint); }
    if (method === 'DELETE') { state.endpoints = []; return json(route, { ok: true }); }
    return json(route, { error: 'Unexpected request' }, 500);
  });
  try {
    await page.getByRole('button', { name: 'Webhooks', exact: true }).click();
    await page.getByText('No webhook endpoints yet.', { exact: true }).waitFor();
    await page.getByLabel('Endpoint URL', { exact: true }).fill(endpoint.url);
    await page.getByLabel('Description (optional)', { exact: true }).fill('CRM sync');
    await page.getByLabel('Google Ads accepted a conversion conversion.delivered', { exact: true }).check();
    await page.getByLabel('Include contact details: name, phone or email, notes, and form fields', { exact: true }).uncheck();
    await page.getByRole('button', { name: 'Add endpoint', exact: true }).click();
    assert.equal(await page.getByLabel('Webhook signing secret', { exact: true }).inputValue(), 'whsec_fixturesecretnotreal000000000000000000=');
    assert(await page.getByRole('button', { name: 'Add endpoint', exact: true }).isDisabled(), 'One secret on screen at a time');
    await page.getByRole('button', { name: 'I saved the secret', exact: true }).click();
    assert.equal(await page.getByLabel('Webhook signing secret', { exact: true }).count(), 0);
    const card = page.getByRole('article', { name: 'Webhook CRM sync', exact: true });
    await card.getByText('Contact details left out').waitFor();

    await card.getByRole('button', { name: 'Send test', exact: true }).click();
    const failed = card.getByRole('listitem', { name: 'Delivery webhook.test failed', exact: true });
    await failed.getByText('The endpoint returned 400').waitFor();
    await failed.getByRole('button', { name: 'Redeliver', exact: true }).click();
    await card.getByRole('listitem', { name: 'Delivery webhook.test succeeded', exact: true }).waitFor();

    await card.getByRole('button', { name: 'Pause', exact: true }).click();
    await card.getByRole('button', { name: 'Resume', exact: true }).waitFor();
    assert(await card.getByRole('button', { name: 'Send test', exact: true }).isDisabled());
    await card.getByRole('button', { name: 'Resume', exact: true }).click();
    await card.getByRole('button', { name: 'Pause', exact: true }).waitFor();

    await card.getByRole('button', { name: 'Rotate secret', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm rotation', exact: true }).click();
    assert.equal(await page.getByLabel('Webhook signing secret', { exact: true }).inputValue(), 'whsec_rotatedsecretnotreal0000000000000000=');
    await card.getByText(/Previous secret also signs until/).waitFor();
    await page.getByRole('button', { name: 'I saved the secret', exact: true }).click();

    await page.screenshot({ path: path.join(artifactDir, 'webhooks-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Webhooks must fit mobile');
    await page.screenshot({ path: path.join(artifactDir, 'webhooks-mobile.png'), fullPage: true });
    await page.setViewportSize({ width: 1440, height: 1000 });

    await card.getByRole('button', { name: 'Delete', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await page.getByText('No webhook endpoints yet.', { exact: true }).waitFor();
    assert(!state.requests.some(r => r.startsWith('GET') && r.includes('secret')), 'Secrets are never fetched');
  } finally {
    await page.unroute('**/api/webhook**');
  }
}
