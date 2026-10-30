// UI contract test against the production build; Rails tests verify real accounting.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
const port = 45129;
const origin = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "preview", "--host", "127.0.0.1", "--port", String(port), "--strictPort"], { cwd: new URL("..", import.meta.url), stdio: "ignore" });
const id = "11111111-2222-4333-8444-555555555555";
const workspace = { id, name: "Billing fixture", status: "ready", paid_access: true, can_manage_billing: true, billing_required: true };
const pricing = { number_month_cents: 200, minute_cents: 4, credit_cents: 2750 };
const period = { id: 1, starts_at: "2026-09-17T00:00:00Z", ends_at: "2026-10-17T00:00:00Z", status: "sealed", invoice_status: "posted", observed_at: "2026-10-18T00:00:00Z", reserved_cents: 0,
  entries: [{ kind: "usage", amount_cents: 4400 }, { kind: "credit", amount_cents: -2750 }], estimate: { gross_cents: 4400, credit_cents: 2750, amount_cents: 1650 } };
let browser;
try {
  for (let i = 0; i < 60; i++) {
    if (server.exitCode !== null) throw new Error("Preview server exited");
    if (await fetch(origin).then(r => r.ok).catch(() => false)) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  let usage = { enrolled: true, currency: "usd", pricing, monthly_limit_cents: 10000, periods: [period] };
  let unavailable = false;
  await page.route("**/runtime.json", route => route.fulfill({ json: { hosted_app_origin: origin } }));
  await page.route("**/api/cloud/**", route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/cloud/session") return route.fulfill({ json: { csrf_token: "fixture", user: { id: "owner", email: "fixture@example.test", email_verified: true }, signups_enabled: true, billing: { enabled: true, mode: "test", amount_minor: 2750, currency: "usd" } } });
    if (path.endsWith("/billing/usage")) return route.fulfill(unavailable ? { status: 503, json: { error: "Unavailable" } } : { json: usage });
    if (path.endsWith("/billing")) return route.fulfill({ json: { workspace, required: true, enabled: true, mode: "test", amount_minor: 2750, currency: "usd", status: "active", paid_access: true, cancellation_scheduled: false, access_until: null, can_checkout: false, can_manage: true } });
    if (path === `/api/cloud/workspaces/${id}`) return route.fulfill({ json: workspace });
    throw new Error(`Unexpected API request ${path}`);
  });
  await mkdir("artifacts/billing-usage", { recursive: true });
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`${origin}/workspaces/${id}/billing`);
    await page.getByRole("heading", { name: "Managed numbers and calls" }).waitFor();
    assert.match(await page.locator(".managed-usage").innerText(), /\$2\.00 per number\/month · \$0\.04 per minute/);
    assert.equal(await page.locator(".managed-usage-period dd").allTextContents().then(x => x.join("|")), "$44.00|−$27.50|$16.50");
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    await page.screenshot({ path: `artifacts/billing-usage/${width}.png`, fullPage: true });
  }
  // A corrected estimate must never replace the sealed invoice values in the UI.
  usage = { ...usage, periods: [{ ...period, status: "review", invoice_status: "review", estimate: { gross_cents: 9900, credit_cents: 2750, amount_cents: 7150 } }] };
  await page.getByRole("button", { name: "Refresh usage", exact: true }).click();
  await page.getByText(/Needs review —/).waitFor();
  assert.equal(await page.locator(".managed-usage-period dd").last().innerText(), "$16.50");
  unavailable = true;
  await page.getByRole("button", { name: "Refresh usage", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: "Usage billing is temporarily unavailable" }).waitFor();
  await page.getByRole("button", { name: "Manage billing", exact: true }).waitFor();
  unavailable = false;
  usage = { ...usage, enrolled: false, periods: [] };
  await page.getByRole("button", { name: "Refresh usage", exact: true }).click();
  await page.getByRole("heading", { name: "Managed numbers and calls" }).waitFor({ state: "detached" });
  assert.deepEqual(errors, []);
  console.log("PASS: usage totals, sealed corrections, desktop/mobile layout, unavailable API and unenrolled workspace");
} finally {
  await browser?.close();
  server.kill();
}
