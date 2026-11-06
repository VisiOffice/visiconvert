import { useEffect, useRef, useState } from "react";
import type { Client } from "./api";

type Check = { valid: boolean; message: string; destination_version: number; account?: { id: string; name: string; status: string }; mapped?: { action: string; goal_name: string; found: boolean; eligible: boolean; accepts_uploads_after: string | null }[] };
type Status = { configured: boolean; connected: boolean; reconnect_required: boolean; revision: number; redirect_uri: string; customer_id: string | null; account_id: string | null; account_name: string | null; destination_version: number; validation: Check | null; checked_at: string | null };
type Account = { id: string; name: string; number: string; customer_id: string; status: string; currency: string };
type Goal = { id: string; name: string; status: string; kind: string; category: string; count_type: string; window_minutes: number; eligible: boolean };
type Action = { key: string; name: string; enabled: boolean; counting: string; version: number; microsoft_goal_name?: string | null };
type Setup = { ready: boolean; summary: string; next_steps: { actor: string; step: string; advisory?: boolean }[] };
type Overview = { actions: Action[]; setup: { microsoft?: Setup } };
type Review = { summary: string; warnings: { id: string; name: string; flags: string[] }[]; flags: Record<string, string> };

/// Microsoft Advertising as a second destination for the same pipeline actions. `revision`
/// is the shared delivery settings version; `onChange` refreshes the rest of the page.
export function MicrosoftConnection({ api, revision, onChange }: { api: Client; revision: number; onChange: () => Promise<void> }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [moreAccounts, setMoreAccounts] = useState(false);
  const [chosen, setChosen] = useState("");
  const [goals, setGoals] = useState<Goal[] | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  const [busyCount, setBusyCount] = useState(0);
  const busy = busyCount > 0;
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const cleanup = useRef<() => void>(() => {});
  async function refresh() {
    const [s, o] = await Promise.all([api<Status>("/connections/microsoft"), api<Overview>("/conversions/pipeline")]);
    setStatus(s); setOverview(o); return s;
  }
  useEffect(() => { let live = true; Promise.all([api<Status>("/connections/microsoft"), api<Overview>("/conversions/pipeline")]).then(([s, o]) => { if (live) { setStatus(s); setOverview(o); } }).catch(e => { if (live) setError(e.message); }); return () => { live = false; cleanup.current(); }; }, [api, revision]);
  async function run(fn: () => Promise<void>) {
    setBusyCount(n => n + 1); setError(""); setNotice("");
    try { await fn(); } catch (e) { setError((e as Error).message); }
    finally { await refresh().catch(() => {}); setBusyCount(n => n - 1); }
  }
  function connect() {
    if (!status) return;
    // Open synchronously so normal popup blockers allow the consent window.
    const popup = window.open("about:blank", "visiconvert-microsoft", "popup,width=640,height=760");
    if (!popup) { setError("Allow popups for this site, then connect Microsoft Advertising again."); return; }
    cleanup.current(); setPending(true);
    const start = status.revision;
    let stopped = false; let timer: ReturnType<typeof setTimeout> | undefined;
    const stop = () => { stopped = true; clearTimeout(timer); window.removeEventListener("message", message); setPending(false); };
    const done = "Microsoft sign-in complete. Choose the ad account below.";
    const message = (event: MessageEvent) => {
      if (event.origin !== window.location.origin || event.source !== popup || event.data?.type !== "visiconvert-microsoft-result") return;
      stop();
      void refresh().then(s => {
        if (s.connected && s.revision > start) { setNotice(done); setError(""); }
        else setError("Microsoft Advertising was not connected. Try again and accept the requested permission. Ask your administrator to check the Microsoft application configuration if it keeps failing.");
      }).catch(e => setError(e.message));
    };
    cleanup.current = stop; window.addEventListener("message", message);
    void run(async () => {
      try {
        const result = await api<{ authorization_url: string }>("/connections/microsoft/start", { revision: start });
        const u = new URL(result.authorization_url);
        if (u.origin !== "https://login.microsoftonline.com") throw new Error("Unexpected Microsoft authorization address.");
        popup.location.href = u.href;
        setNotice("Finish connecting in the Microsoft window.");
        let tries = 0;
        const poll = async () => {
          if (stopped) return;
          try { const s = await refresh(); if (s.connected && s.revision > start) { stop(); popup.close(); setNotice(done); return; } }
          catch { /* The refresh control remains available. */ }
          if (++tries < 300) timer = setTimeout(() => void poll(), 2000); else { stop(); setNotice("Connection window expired. Refresh or try again."); }
        };
        timer = setTimeout(() => void poll(), 2000);
      } catch (e) { stop(); popup.close(); throw e; }
    });
  }
  async function loadAccounts() {
    setAccounts(null);
    const r = await api<{ accounts: Account[]; has_more: boolean }>("/connections/microsoft/accounts");
    setAccounts(r.accounts); setMoreAccounts(r.has_more);
    setChosen(c => c || status?.account_id || (r.accounts.length === 1 ? r.accounts[0].id : ""));
  }
  async function loadGoals() {
    setGoals(null);
    const r = await api<{ goals: Goal[] }>("/connections/microsoft/goals");
    setGoals(r.goals);
  }
  useEffect(() => {
    if (status?.connected) void run(loadAccounts);
    else { setAccounts(null); setGoals(null); setReview(null); setChosen(""); }
  }, [api, status?.connected, status?.revision]);
  useEffect(() => { if (status?.connected && status.account_id) void run(loadGoals); else setGoals(null); }, [status?.connected, status?.account_id]);
  async function changed(message: string) { await onChange(); setNotice(message); }
  const setup = overview?.setup.microsoft;
  const actions = overview?.actions.filter(a => a.enabled) ?? [];
  const validation = status?.validation?.destination_version === status?.destination_version ? status?.validation : null;
  const offline = goals?.filter(g => g.kind === "OfflineConversion") ?? [];
  return <section className="card site-panel microsoft-connection" aria-label="Microsoft Advertising">
    <div className="section-heading"><div><span className="eyebrow">OPTIONAL · MICROSOFT ADVERTISING</span><h2>Send to Microsoft Advertising</h2></div><button className="secondary" disabled={busy} onClick={() => void run(async () => { await refresh(); setNotice("Microsoft connection refreshed."); })}>Refresh Microsoft connection</button></div>
    <p className="muted">The same actions and events can also go to Microsoft Advertising (Bing). Events with a Microsoft click ID (<code>msclkid</code>) are sent to the offline conversion goal you map each action to. Delivery, automatic sending and the consent policy on the Overview tab are shared with Google. Only events recorded after an action is mapped are sent, so an older uploader can be switched off without double counting.</p>
    {error && <p className="error" role="alert">{error}</p>}{notice && <p className="notice" role="status">{notice}</p>}
    {!status ? <p>Loading Microsoft connection…</p> : !status.configured ? <>
      <p>Microsoft Advertising is not available on this installation.</p>
      <details><summary>Administrator setup</summary><p>Register a multitenant Microsoft Entra application with this exact Web redirect URI:</p><code className="google-callback">{status.redirect_uri}</code><p>Set <code>VISICONVERT_MICROSOFT_CLIENT_ID</code>, <code>VISICONVERT_MICROSOFT_CLIENT_SECRET</code> and <code>VISICONVERT_MICROSOFT_DEVELOPER_TOKEN</code> on the server, all three or none. See the Microsoft connection guide.</p></details>
    </> : <>
      <p><strong>{status.connected ? "Microsoft sign-in complete" : status.reconnect_required ? "Reconnect Microsoft after the application configuration change" : "Microsoft Advertising not connected"}</strong>{status.account_id && <> · Saved account <strong>{status.account_name || status.account_id}</strong> ({status.account_id})</>}</p>
      <div className="google-actions"><button className="primary" disabled={busy || pending} onClick={connect}>{status.connected ? "Use a different Microsoft login" : "Connect Microsoft Advertising"}</button>
        {(status.connected || status.revision > 0 || pending) && <button className="secondary" disabled={busy} onClick={() => void run(async () => { await api("/connections/microsoft", { revision: status.revision }, "DELETE"); cleanup.current(); await changed("Microsoft disconnected. Nothing more is sent to Microsoft until you reconnect; the saved account and goal mappings are kept."); })}>Disconnect Microsoft</button>}</div>
      {status.connected && <div className="site-form">
        <h3>Ad account</h3>
        {!accounts ? <p role="status">Loading your Microsoft Advertising accounts…</p> : !accounts.length ? <p>No ad accounts are reachable with this Microsoft login. Sign in with a user who has access to the advertising account.</p> : <>
          <div className="form-field"><label htmlFor="microsoft-account">Microsoft Advertising account</label><select id="microsoft-account" disabled={busy} value={chosen} onChange={e => setChosen(e.target.value)}><option value="">Choose an account</option>{accounts.map(a => <option key={a.id} value={a.id} disabled={a.status !== "Active"}>{a.name} · {a.number} · ID {a.id}{a.status !== "Active" ? ` · ${a.status}` : ""}</option>)}</select></div>
          {moreAccounts && <p className="muted">Showing the first accounts returned. If yours is missing, ask an agent to set it by ID.</p>}
          {chosen && chosen !== status.account_id && <div><button className="primary" disabled={busy} onClick={() => void run(async () => { await api("/conversion-setup/microsoft-destination", { revision: status.revision, destination_version: status.destination_version, account_id: chosen }, "PUT"); await changed("Microsoft ad account saved. Map your actions to its goals below."); })}>{status.account_id ? "Replace saved account" : "Save ad account"}</button></div>}
        </>}
      </div>}
      {status.connected && status.account_id && <div className="site-form">
        <h3>Goal for each action</h3>
        <p className="muted">Uploads use the goal’s exact name. Choose an existing offline conversion goal, or create one named after the action. Microsoft accepts uploads for a new goal about two hours after it is created; eligible events wait until then. Creating a goal turns on <code>msclkid</code> auto-tagging and counts the goal for bidding.</p>
        {!goals ? <p role="status">Loading Microsoft goals…</p> : !actions.length ? <p>Add an enabled action on the Actions tab first.</p> : actions.map(a => <GoalRow key={`${a.key}:${a.version}`} action={a} goals={offline} busy={busy}
          map={name => run(async () => { await api(`/conversion-actions/${a.key}/microsoft-goal`, { version: a.version, goal_name: name }, "PUT"); await changed(name ? `Action ${a.key} mapped to ${name}. Events recorded from now on are sent to Microsoft.` : `Action ${a.key} is no longer sent to Microsoft.`); })}
          create={() => run(async () => { await api(`/conversion-actions/${a.key}/microsoft-goal`, { version: a.version }, "POST"); await loadGoals(); await changed(`Goal created or recovered for ${a.key}. Microsoft accepts uploads for it in about two hours.`); })} />)}
        <div className="google-actions">
          <button className="secondary" disabled={busy} onClick={() => void run(loadGoals)}>Refresh goals</button>
          <button className="secondary" disabled={busy} onClick={() => void run(async () => { const r = await api<Check>("/conversion-setup/microsoft-check", { revision: status.revision, destination_version: status.destination_version }); setNotice(r.message); })}>Check Microsoft destination</button>
          <button className="secondary" disabled={busy} onClick={() => void run(async () => setReview(await api<Review>("/conversion-setup/microsoft-review")))}>Review other goals</button>
        </div>
      </div>}
      {validation && <div className={validation.valid ? "notice" : "error"}><strong>{validation.valid ? "Microsoft destination checks passed" : "Microsoft destination needs attention"}</strong><p>{validation.message}</p>{validation.mapped?.filter(m => !m.eligible).map(m => <p key={m.action}><code>{m.action}</code>: goal “{m.goal_name}” {m.found ? "is not an active offline conversion goal" : "was not found in this account"}.</p>)}{status.checked_at && <small>Checked {new Date(status.checked_at).toLocaleString()}</small>}</div>}
      {review && <div className={review.warnings.length ? "error" : "notice"}><strong>Other goals in this account</strong><p>{review.summary}</p>{review.warnings.length > 0 && <ul>{review.warnings.map(w => <li key={w.id}>{w.name}: {w.flags.map(f => review.flags[f] ?? f).join(" ")}</li>)}</ul>}</div>}
    </>}
    {setup && <div className={setup.ready ? "notice" : "muted"} aria-label="Microsoft setup checklist"><strong>{setup.ready ? "Microsoft ready" : "Microsoft: still to do"}</strong>{setup.ready && <p>{setup.summary}</p>}{setup.next_steps.length > 0 && <ul>{setup.next_steps.map(s => <li key={s.step}>{s.advisory ? "Note: " : ""}{s.step}</li>)}</ul>}</div>}
  </section>;
}

function GoalRow({ action, goals, busy, map, create }: { action: Action; goals: Goal[]; busy: boolean; map: (name: string | null) => Promise<void>; create: () => Promise<void> }) {
  const saved = action.microsoft_goal_name ?? "";
  const [draft, setDraft] = useState(saved);
  const id = `microsoft-goal-${action.key}`;
  const known = goals.some(g => g.name === saved);
  return <div className="form-field">
    <label htmlFor={id}>{action.name} · <code>{action.key}</code></label>
    <select id={id} disabled={busy} value={draft} onChange={e => setDraft(e.target.value)}>
      <option value="">Not sent to Microsoft</option>
      {saved && !known && <option value={saved}>{saved} (not found in this account)</option>}
      {goals.map(g => <option key={g.id} value={g.name} disabled={!g.eligible}>{g.name}{g.eligible ? "" : ` · ${g.status}`}</option>)}
    </select>
    <div className="site-actions">
      {draft !== saved && <button className="primary" disabled={busy} onClick={() => void map(draft || null)}>{draft ? "Save goal" : "Stop sending to Microsoft"}</button>}
      {!saved && <button className="secondary" disabled={busy} onClick={() => void create()}>Create goal in Microsoft</button>}
    </div>
  </div>;
}
