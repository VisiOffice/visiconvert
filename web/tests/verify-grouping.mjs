import assert from 'node:assert/strict';
import path from 'node:path';

export async function verifyGrouping({ page, api, artifactDir }) {
  const created = [];
  for (const [name, source, notes] of [
    ['Grouping initial request', 'Grouping Google', 'Initial quote request.'],
    ['Grouping follow-up', 'Grouping Referral', 'Follow-up availability.'],
  ]) {
    const response = await api('/leads', { external_id: name, name, contact: 'same@example.com', source, campaign: '', notes });
    assert.equal(response.status, 200);
    created.push((await api('/inquiries')).body.find(i => i.name === name));
  }
  const [target, source] = created;
  assert.notEqual(target.id, source.id, 'Same contact must not automatically merge');
  for (const [inquiry, amount_minor] of [[target, 10000], [source, 25005]]) {
    assert.equal((await api(`/inquiries/${inquiry.id}/outcomes`, { external_id: `booking-${inquiry.id}`, amount_minor, currency: 'USD', status: 'booked', revision: 1 }, 'PUT')).status, 200);
  }
  await page.getByRole('button', { name: 'Refresh data', exact: true }).click();
  await page.getByRole('button', { name: /Lead inbox/ }).click();
  await page.getByLabel('Search leads').fill('Grouping follow-up');
  await page.getByRole('button', { name: /Grouping follow-up/ }).click();
  await page.getByText('Combine with another inquiry', { exact: true }).click();
  await page.getByLabel('Destination inquiry', { exact: true }).selectOption(target.id);
  await page.getByLabel('Reason for combining').fill('Same quote request, confirmed with customer.');
  await page.getByRole('button', { name: 'Combine inquiries', exact: true }).click();
  await page.getByRole('heading', { name: '2 interactions', exact: true }).waitFor();
  assert.equal(await page.getByLabel('Notes', { exact: true }).inputValue(), target.notes);
  const combined = (await api(`/inquiries/${target.id}`)).body;
  assert.equal(combined.interactions.length, 2);
  assert.equal(combined.inquiry.booked_minor, 35005);
  assert.equal(combined.inquiry.source, target.source);
  assert.equal((await api(`/inquiries/${source.id}`)).status, 404);
  // Searching for the follow-up must still find the destination inquiry.
  assert.equal(await page.locator('tbody tr').count(), 1);
  await page.getByText('Inquiry history', { exact: true }).click();
  await page.getByText('Same quote request, confirmed with customer.', { exact: true }).waitFor();
  await page.getByText('Previous inquiry review', { exact: true }).click();
  await page.getByText('Follow-up availability.', { exact: true }).last().waitFor();
  await page.getByRole('button', { name: /Earlier inquiry history/ }).click();
  await page.getByRole('heading', { name: 'Earlier inquiry history', exact: true }).waitFor();
  await page.getByLabel('Qualification', { exact: true }).selectOption('qualified');
  await page.getByLabel('Notes', { exact: true }).fill('Combined inquiry reviewed.');
  assert(await page.getByRole('button', { name: 'Separate this interaction' }).first().isDisabled(), 'Unsaved review must block grouping');
  await page.getByRole('button', { name: 'Save lead', exact: true }).click();
  await page.getByRole('status').filter({ hasText: 'Saved' }).waitFor();
  const reports = () => api('/summary').then(r => r.body.filter(s => s.source.startsWith('Grouping ')));
  let rows = await reports();
  assert.equal(rows.length, 1);
  assert.equal(rows[0].leads, 1);
  assert.equal(rows[0].qualified, 1);
  assert.equal(rows[0].booked_minor, 35005);
  await page.getByRole('heading', { name: '2 interactions', exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(artifactDir, 'inquiry-grouped-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('heading', { name: '2 interactions', exact: true }).scrollIntoViewIfNeeded();
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Inquiry drawer must fit mobile');
  await page.screenshot({ path: path.join(artifactDir, 'inquiry-grouped-mobile.png'), fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.locator('article.interaction').filter({ hasText: 'Grouping follow-up' }).getByRole('button', { name: 'Separate this interaction' }).click();
  await page.getByLabel('Reason for separating').fill('Customer confirmed a second opportunity.');
  await page.getByRole('button', { name: 'Create separate inquiry', exact: true }).click();
  // Refreshing the original inquiry also briefly shows one interaction. Wait
  // for selection to finish switching to the newly created inquiry first.
  await page.getByRole('dialog', { name: 'Lead details', exact: true }).getByRole('heading', { name: 'Grouping follow-up', exact: true }).waitFor();
  await page.getByRole('heading', { name: '1 interaction', exact: true }).waitFor();
  assert.equal(await page.getByLabel('Qualification', { exact: true }).inputValue(), 'unreviewed');
  assert.equal(await page.getByLabel('Notes', { exact: true }).inputValue(), '');
  await page.getByText('$250.05 · booked · revision 1', { exact: true }).waitFor();
  rows = await reports();
  assert.equal(rows.length, 2);
  assert.equal(rows.reduce((n, s) => n + s.leads, 0), 2);
  assert.equal(rows.reduce((n, s) => n + s.booked_minor, 0), 35005);
  assert.equal(rows.find(s => s.source === source.source).booked_minor, 25005);
  const remaining = (await api(`/inquiries/${target.id}`)).body.inquiry;
  assert.equal(remaining.notes, 'Combined inquiry reviewed.');
  assert.equal(remaining.booked_minor, 10000);
  await page.screenshot({ path: path.join(artifactDir, 'inquiry-separated.png'), fullPage: true });
  await page.getByRole('button', { name: 'Close details' }).click();
  await page.getByLabel('Search leads').fill('');
}
