import assert from "node:assert/strict";
import { verifyGrouping } from "./verify-grouping.mjs";
import { verifyDni } from "./verify-dni.mjs";
import { verifySites } from "./verify-sites.mjs";
import { randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createWriteStream, existsSync } from "node:fs";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir, userInfo } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { chromium } from "playwright";

// Uses a new PostgreSQL cluster and real production assets/API. No mocked routes.
// Never loads the development API key or uses its database/provider credentials.
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const runId = `${new Date().toISOString().replaceAll(/[:.]/g, "-")}-${process.pid}`;
const artifactDir = path.join(root, "artifacts", "verification", runId);
await mkdir(artifactDir, { recursive: true, mode: 0o700 });
const scratch = await mkdtemp(path.join(tmpdir(), "vc-verify-"));
const pgData = path.join(scratch, "pg");
const socketDir = path.join(scratch, "socket");
await mkdir(socketDir, { mode: 0o700 });
const apiKey = randomBytes(32).toString("hex");
const workspace = "820bcdbe-951c-4ccc-8853-3cb991e94a01";
const databaseURL = `postgresql:///visiconvert_test?host=${encodeURIComponent(socketDir)}`;
const env = {
  // Remove inherited libpq settings instead of blanking them. PGSERVICE=""
  // requests a service literally named ""; PGHOSTADDR can override -h.
  ...Object.fromEntries(
    Object.entries(process.env).filter(([name]) => !name.startsWith("PG")),
  ),
  DATABASE_URL: databaseURL,
  VISICONVERT_TEST_DATABASE_URL: databaseURL,
  VISICONVERT_WORKSPACE_ID: workspace,
  VISICONVERT_API_KEY: apiKey,
  VISICONVERT_ENCRYPTION_KEY: "",
  VISICONVERT_CLOUD_SERVICE_KEY: "",
  VISICONVERT_CLOUD_ORIGIN: "",
  TWILIO_ACCOUNT_SID: "",
  TWILIO_AUTH_TOKEN: "",
  PGCONNECT_TIMEOUT: "5",
  PGPORT: "5432",
  PGUSER: userInfo().username,
};
const log = createWriteStream(path.join(artifactDir, "services.log"), {
  flags: "a",
  mode: 0o600,
});
const result = {
  status: "running",
  started_at: new Date().toISOString(),
  checks: [],
  artifacts: artifactDir,
};
let postgresStarted = false;
let apiProcess;
let browser;
let page;
let origin;
let activeCommand;

async function command(executable, args, options = {}) {
  const child = spawn(executable, args, {
    cwd: root,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
  activeCommand = child;
  child.stdout.pipe(log, { end: false });
  child.stderr.pipe(log, { end: false });
  try {
    const [code, signal] = await once(child, "close");
    if (code !== 0)
      throw new Error(
        `${executable} failed (${signal || code}); see services.log and postgres.log`,
      );
  } finally {
    activeCommand = undefined;
  }
}

async function check(name, fn) {
  process.stdout.write(`${name}… `);
  await fn();
  result.checks.push(name);
  console.log("PASS");
}

async function api(route, data, method = data ? "POST" : "GET", key = apiKey) {
  const response = await fetch(`${origin}/api${route}`, {
    method,
    headers: {
      Authorization: `Bearer ${key}`,
      ...(data ? { "Content-Type": "application/json" } : {}),
    },
    body: data ? JSON.stringify(data) : undefined,
    signal: AbortSignal.timeout(5000),
  });
  const body = await response.json();
  return { status: response.status, body };
}

async function waitUntil(fn, description) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (await fn()) return;
    await delay(100);
  }
  throw new Error(`Timed out: ${description}`);
}

async function startApi() {
  apiProcess = spawn(
    path.join(root, "target/debug/convert-server"),
    ["serve"],
    {
      cwd: root,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let spawnError;
  apiProcess.once("error", (error) => {
    spawnError = error;
  });
  apiProcess.stdout.pipe(log, { end: false });
  apiProcess.stderr.pipe(log, { end: false });
  await waitUntil(async () => {
    if (spawnError) throw spawnError;
    if (apiProcess.exitCode !== null || apiProcess.signalCode !== null)
      throw new Error("API exited before startup; see services.log");
    try {
      const response = await api("/bootstrap");
      return response.status === 200 && response.body.id === workspace;
    } catch {
      return false;
    }
  }, "API startup");
}

async function stopApi() {
  const child = apiProcess;
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const stopped = once(child, "exit");
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  try {
    await stopped;
  } finally {
    clearTimeout(timer);
    apiProcess = undefined;
  }
}

// Cleanup targets only child processes started by this verification run.
async function cleanup() {
  if (activeCommand?.exitCode === null) activeCommand.kill("SIGTERM");
  await browser?.close();
  await stopApi();
  if (postgresStarted) {
    await command("pg_ctl", ["-D", pgData, "-m", "fast", "-w", "stop"]);
    postgresStarted = false;
  }
}
process.once("SIGINT", () => {
  void cleanup().finally(() => process.exit(130));
});
process.once("SIGTERM", () => {
  void cleanup().finally(() => process.exit(143));
});

try {
  await check("Tracker privacy, consent withdrawal, blocked storage, and retry behavior", () => command("node", ["--test", "web/tests/tracker.test.mjs"]));
  await check(
    "Start isolated PostgreSQL and create test database",
    async () => {
      await command("initdb", [
        "-D",
        pgData,
        "-A",
        "trust",
        "--no-locale",
        "-E",
        "UTF8",
      ]);
      await command("pg_ctl", [
        "-D",
        pgData,
        "-l",
        path.join(artifactDir, "postgres.log"),
        "-o",
        `-k ${socketDir} -h ''`,
        "-w",
        "start",
      ]);
      postgresStarted = true;
      await command("createdb", ["-h", socketDir, "visiconvert_test"]);
    },
  );
  await check(
    "Postgres integration: retries, isolation, revisions, call ordering",
    () =>
      command("cargo", [
        "test",
        "-p",
        "convert-store",
        "--test",
        "persistence",
        "--",
        "--ignored",
        "--test-threads=1",
      ]),
  );
  await check("Hosted handoff: membership, browser binding, replay, expiry, and isolation", async () => {
    await command("cargo", ["test", "-p", "convert-server", "hosted_tests::", "--", "--ignored", "--test-threads=1"]);
  });
  await check("Twilio connections: encrypted credentials, ownership, and workspace callback isolation", async () => {
    await command("cargo", ["test", "-p", "convert-server", "connection_tests::shared_connections", "--", "--ignored", "--test-threads=1"]);
  });
  await check("Website collection: consent, retries, rate limits, and workspace isolation", async () => {
    await command("cargo", ["test", "-p", "convert-server", "site_tests::", "--", "--ignored", "--test-threads=1"]);
  });
  await check("Inquiry grouping: migration, membership, callbacks, history, and booking totals", async () => {
    await command("cargo", ["test", "-p", "convert-store", "--test", "inquiry_upgrade", "--", "--ignored", "--test-threads=1"]);
    await command("cargo", ["test", "-p", "convert-server", "inquiry_tests::", "--", "--ignored", "--test-threads=1"]);
  });
  await check("DNI allocation: concurrency, cooldown, reuse, callback evidence, and isolation", () => command("cargo", ["test", "-p", "convert-store", "--test", "dni", "--", "--ignored", "--test-threads=1"]));
  await check("Apply application migrations twice", async () => {
    await command(path.join(root, "target/debug/convert-server"), ["migrate"]);
    await command(path.join(root, "target/debug/convert-server"), ["migrate"]);
  });
  await check("Start real Rust API with production frontend", async () => {
    const reservation = createServer();
    reservation.listen(0, "127.0.0.1");
    await once(reservation, "listening");
    const port = reservation.address().port;
    await new Promise((resolve, reject) =>
      reservation.close((error) => (error ? reject(error) : resolve())),
    );
    origin = `http://127.0.0.1:${port}`;
    env.PUBLIC_URL = origin;
    env.LISTEN_ADDR = `127.0.0.1:${port}`;
    await startApi();
  });
  await check("Private API rejects an incorrect key", async () => {
    assert.equal(
      (await api("/leads", undefined, "GET", "wrong-key")).status,
      401,
    );
    assert.deepEqual((await api("/leads")).body, []);
  });
  const executablePath =
    process.env.CHROMIUM_PATH ||
    (existsSync("/usr/bin/chromium") ? "/usr/bin/chromium" : undefined);
  browser = await chromium.launch({ headless: true, executablePath });
  page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
  });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await check("Browser login and empty overview", async () => {
    await page.goto(origin);
    await page
      .getByLabel("Workspace API key")
      .fill("invalid-key-long-enough-for-validation");
    await page.getByRole("button", { name: "Open workspace" }).click();
    await page.getByRole("alert").waitFor();
    await page.getByLabel("Workspace API key").fill(apiKey);
    await page.getByRole("button", { name: "Open workspace" }).click();
    await page.getByRole("heading", { name: "See what’s working." }).waitFor();
    await page.getByRole("heading", { name: "No inquiries to show" }).waitFor();
  });
  let lead;
  await check("Create a form lead through the browser", async () => {
    await page
      .getByRole("button", { name: "Add form lead", exact: true })
      .click();
    const form = page.getByRole("dialog");
    for (const [label, value] of [
      ["Submission ID", "browser-submission-1"],
      ["Name", "Browser verification"],
      ["Email or phone", "verification@example.com"],
      ["Marketing source", "Verification"],
      ["Campaign (optional)", "Local browser check"],
    ])
      await form.getByLabel(label, { exact: true }).fill(value);
    await form.getByRole("button", { name: "Add lead", exact: true }).click();
    await page.getByRole("button", { name: /Browser verification/ }).waitFor();
    const response = await api("/leads");
    assert.equal(response.body.length, 1);
    lead = response.body[0];
    assert.equal(lead.source, "Verification");
    assert.equal(lead.is_demo, false);
  });
  await check("Qualify lead and save notes through the browser", async () => {
    await page.getByRole("button", { name: /Browser verification/ }).click();
    await page
      .getByLabel("Qualification", { exact: true })
      .selectOption("qualified");
    await page
      .getByLabel("Notes", { exact: true })
      .fill("Requested an appointment.");
    await page.getByRole("button", { name: "Save lead", exact: true }).click();
    await page.getByRole("status").filter({ hasText: "Saved" }).waitFor();
    lead = (await api("/leads")).body[0];
    assert.equal(lead.qualification, "qualified");
    assert.equal(lead.notes, "Requested an appointment.");
    assert.equal(lead.version, 2);
  });
  const booking = {
    external_id: "browser-booking-1",
    amount_minor: 25005,
    currency: "USD",
    status: "booked",
    revision: 1,
  };
  await check(
    "Record exact-cent booking and verify source revenue",
    async () => {
      await page
        .getByLabel("Booking ID", { exact: true })
        .fill(booking.external_id);
      await page.getByLabel("Amount (USD)", { exact: true }).fill("250.05");
      await page.getByRole("button", { name: "Record booking" }).click();
      await page
        .getByText("$250.05 · booked · revision 1", { exact: true })
        .waitFor();
      assert.deepEqual((await api(`/leads/${lead.id}/outcomes`)).body, [
        booking,
      ]);
      assert.equal((await api("/summary")).body[0].booked_minor, 25005);
      assert.equal(
        (await api(`/leads/${lead.id}/outcomes`, booking, "PUT")).status,
        200,
      );
      assert.equal((await api("/summary")).body[0].bookings, 1);
      assert.equal((await api("/summary")).body[0].booked_minor, 25005);
    },
  );
  await check(
    "Restart API and database; verify persisted lead and booking",
    async () => {
      await stopApi();
      await command("pg_ctl", ["-D", pgData, "-m", "fast", "-w", "stop"]);
      postgresStarted = false;
      await command("pg_ctl", [
        "-D",
        pgData,
        "-l",
        path.join(artifactDir, "postgres.log"),
        "-o",
        `-k ${socketDir} -h ''`,
        "-w",
        "start",
      ]);
      postgresStarted = true;
      await startApi();
      await page.reload();
      await page.getByLabel("Workspace API key").fill(apiKey);
      await page.getByRole("button", { name: "Open workspace" }).click();
      await page.getByRole("button", { name: /Browser verification/ }).click();
      assert.equal(
        await page.getByLabel("Qualification", { exact: true }).inputValue(),
        "qualified",
      );
      assert.equal(
        await page.getByLabel("Notes", { exact: true }).inputValue(),
        "Requested an appointment.",
      );
      await page
        .getByText("$250.05 · booked · revision 1", { exact: true })
        .waitFor();
      assert.equal((await api("/summary")).body[0].booked_minor, 25005);
    },
  );
  await check(
    "Cancel booking through browser; verify zero booked revenue",
    async () => {
      await page.getByRole("button", { name: "Cancel booking" }).click();
      await page
        .getByText("$250.05 · canceled · revision 2", { exact: true })
        .waitFor();
      assert.equal((await api("/summary")).body[0].booked_minor, 0);
      assert.equal((await api("/summary")).body[0].bookings, 0);
      await page.getByRole("button", { name: "Close details" }).click();
    },
  );
  await check(
    "Tracking number registration, search, and real WASM preview",
    async () => {
      await page
        .getByRole("button", { name: "Tracking numbers", exact: true })
        .click();
      await page
        .getByRole("button", { name: "Add tracking number", exact: true })
        .click();
      const form = page.getByRole("dialog");
      for (const [label, value] of [
        ["Label", "Verification line"],
        ["Twilio tracking number", "+12025550105"],
        ["Business destination", "+12025550106"],
        ["Marketing source", "Verification"],
        ["Campaign", "Local browser check"],
      ])
        await form.getByLabel(label, { exact: true }).fill(value);
      await form.getByRole("button", { name: "Register number" }).click();
      await page.getByText("Verification line", { exact: true }).waitFor();
      assert.equal((await api("/numbers")).body[0].destination, "+12025550106");
      await page
        .getByRole("button", { name: "Connections", exact: true })
        .click();
      await page.getByRole("button", { name: "Preview attribution" }).click();
      await page
        .getByRole("status")
        .filter({ hasText: "Source: google · Medium: cpc · Campaign: spring" })
        .waitFor();
      await page.getByRole("button", { name: /Lead inbox/ }).click();
      await page.getByLabel("Search leads").fill("nonexistent lead");
      await page
        .getByRole("heading", { name: "No inquiries to show" })
        .waitFor();
      await page.getByLabel("Search leads").fill("Browser verification");
      assert.equal(await page.locator("tbody tr").count(), 1);
    },
  );
  await check("Desktop/mobile screenshots and browser errors", async () => {
    await page.getByRole("button", { name: "Overview", exact: true }).click();
    await page.screenshot({
      path: path.join(artifactDir, "desktop.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: path.join(artifactDir, "mobile.png"),
      fullPage: true,
    });
    assert.ok(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
      "Mobile page must not overflow horizontally",
    );
    assert.deepEqual(errors, [], "No uncaught browser errors");
  });
  await check(
    "SQL confirms exactly one lead, a canceled booking, and audit history",
    () =>
      command("psql", [
        "-h",
        socketDir,
        "-d",
        "visiconvert_test",
        "-v",
        "ON_ERROR_STOP=1",
        "-c",
        `DO $$ BEGIN
      ASSERT (SELECT count(*) FROM leads WHERE workspace_id='${workspace}') = 1;
      ASSERT (SELECT count(*) FROM outcomes WHERE workspace_id='${workspace}' AND status='canceled' AND revision=2) = 1;
      ASSERT (SELECT count(*) FROM audit_events WHERE workspace_id='${workspace}') = 3;
    END $$;`,
      ]),
  );
  await check("DNI browser flow: pool setup, safe swapping, exhaustion, failures, expiry, and consent", () => verifyDni({ browser, page, origin, api, artifactDir }));
  await check("Website browser flow: setup, consent, successful form, diagnostics, evidence, and pause", () => verifySites({ browser, page, origin, api, artifactDir }));
  assert.deepEqual(errors, [], "No uncaught browser errors after website capture");
  await check("Inquiry browser flow: combine, review, history, separate, and revenue", () => verifyGrouping({ page, api, artifactDir }));
  assert.deepEqual(errors, [], "No uncaught browser errors after grouping");
  result.status = "passed";
} catch (error) {
  result.status = "failed";
  result.error = error.message.replaceAll(apiKey, "[REDACTED]");
  process.exitCode = 1;
  console.error(`\nFAIL: ${result.error}`);
  if (page && !page.isClosed()) {
    try {
      await page.screenshot({
        path: path.join(artifactDir, "failure.png"),
        fullPage: true,
      });
      const snapshot = await page.locator("body").ariaSnapshot();
      await writeFile(
        path.join(artifactDir, "failure-aria.txt"),
        snapshot.replaceAll(apiKey, "[REDACTED]"),
        { mode: 0o600 },
      );
    } catch (diagnosticError) {
      result.diagnostic_error = diagnosticError.message.replaceAll(
        apiKey,
        "[REDACTED]",
      );
    }
  }
} finally {
  try {
    await cleanup();
  } catch (error) {
    result.status = "failed";
    result.cleanup_error = error.message;
    process.exitCode = 1;
    console.error(`Cleanup requires attention: ${error.message}`);
  }
  log.end();
  result.finished_at = new Date().toISOString();
  result.database_files = scratch;
  await writeFile(
    path.join(artifactDir, "result.json"),
    JSON.stringify(result, null, 2) + "\n",
    { mode: 0o600 },
  );
  await writeFile(
    path.join(root, "artifacts", "verification", "latest.json"),
    JSON.stringify(result, null, 2) + "\n",
    { mode: 0o600 },
  );
  console.log(
    `Verification ${result.status}. Report: ${path.join(artifactDir, "result.json")}`,
  );
}
