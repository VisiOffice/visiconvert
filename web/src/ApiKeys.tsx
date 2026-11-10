import { useEffect, useState, type FormEvent } from "react";
import { type Client } from "./api";
type Key = { id: string; name: string; prefix: string; scopes: string[]; expires_at: string | null; revoked_at: string | null; last_used_at: string | null; version: number };
type Data = { keys: Key[]; history: { id: number; kind: string; actor: string; created_at: string }[] };
type Expiry = "never" | "24h" | "7d" | "30d" | "90d" | "custom";
const writeScopes = [
  ["leads:write", "Create interactions and review inquiries"],
  ["bookings:write", "Record, revise, and cancel bookings"],
  ["numbers:write", "Register existing Twilio or SignalWire numbers"],
  ["setup:write", "Create and update websites and DNI pools"],
  ["webhooks:write", "Create and manage webhook endpoints"],
  ["conversions:write", "Define conversion actions, post pipeline events, and map them to Google, Microsoft and Meta"],
];
const highRisk = ["numbers:purchase", "conversions:send"];
const presets: { label: string; name: string; scopes: string[]; expiry: Expiry }[] = [
  { label: "Read only", name: "Reporting", scopes: ["read"], expiry: "never" },
  { label: "Agent setup", name: "Agent setup", scopes: ["read", "setup:write"], expiry: "7d" },
  { label: "Ad platform setup", name: "Ad platform setup", scopes: ["read", "conversions:write", "conversions:send"], expiry: "24h" },
];
const hours: Record<string, number> = { "24h": 24, "7d": 168, "30d": 720, "90d": 2160 };
function status(k: Key) { return k.revoked_at ? "Revoked" : k.expires_at && new Date(k.expires_at) <= new Date() ? "Expired" : "Active"; }
function when(v: string | null, none: string) { return v ? new Date(v).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : none; }

export function ApiKeys({ api, setup = false }: { api: Client; setup?: boolean }) {
  const [data, setData] = useState<Data | null>(null);
  const [open, setOpen] = useState(setup);
  const [name, setName] = useState(setup ? "Agent setup" : "");
  const [scopes, setScopes] = useState(setup ? ["read", "setup:write"] : ["read"]);
  const [expiry, setExpiry] = useState<Expiry>(setup ? "7d" : "never");
  const [custom, setCustom] = useState("");
  const [secret, setSecret] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<{ key: Key; action: "rotate" | "revoke" } | null>(null);
  async function refresh() { setData(await api<Data>("/api-keys")); }
  useEffect(() => { let active = true; api<Data>("/api-keys").then(d => { if (active) { setData(d); if (!d.keys.some(k => status(k) === "Active")) setOpen(true); } }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api]);
  async function change(path: string, body: unknown) {
    setBusy(true); setError(""); setNotice(""); setSecret("");
    try {
      const result = await api<{ secret?: string }>(path, body);
      setSecret(result.secret ?? ""); setConfirmation(null); await refresh();
      if (result.secret) { setOpen(false); setName(""); setScopes(["read"]); setExpiry("never"); setCustom(""); }
      setNotice(result.secret ? "Save this key now. Its secret is shown only once." : "Key revoked. New requests using it are rejected.");
    }
    catch (e) { setError((e as Error).message); }
    finally { setBusy(false); }
  }
  function toggle(scope: string, checked: boolean) {
    setScopes(checked ? [...scopes, scope] : scopes.filter(s => s !== scope));
    // Billable and sending permissions default to a 24-hour key.
    if (checked && highRisk.includes(scope) && (expiry === "never" || (hours[expiry] ?? 0) > 24)) setExpiry("24h");
  }
  function preset(p: typeof presets[number]) { setName(p.name); setScopes(p.scopes); setExpiry(p.expiry); }
  function expiresAt() {
    if (expiry === "never") return null;
    if (expiry === "custom") return custom ? new Date(`${custom}Z`).toISOString() : null;
    return new Date(Date.now() + hours[expiry] * 3600000).toISOString();
  }
  function create(e: FormEvent) { e.preventDefault(); void change("/api-keys", { name, scopes, expires_at: expiresAt() }); }
  const active = data?.keys.filter(k => status(k) === "Active") ?? [];
  const inactive = data?.keys.filter(k => status(k) !== "Active") ?? [];
  const row = (k: Key) => <article className="key-row" key={k.id} aria-label={`API key ${k.name}`}>
    <div className="key-main"><strong>{k.name}</strong><code>{k.prefix}…</code><span className={`badge ${status(k) === "Active" ? "qualified" : ""}`}><i />{status(k)}</span></div>
    <div className="key-scopes">{k.scopes.map(s => <span key={s} className={`chip ${highRisk.includes(s) ? "chip-risk" : ""}`}>{s}</span>)}</div>
    <small className="key-meta">Expires {when(k.expires_at, "never")} · Last used {when(k.last_used_at, "never")}</small>
    {!k.revoked_at && <div className="site-actions"><button className="secondary" disabled={busy || !!secret} onClick={() => setConfirmation({ key: k, action: "rotate" })}>Rotate key</button><button className="secondary" disabled={busy} onClick={() => setConfirmation({ key: k, action: "revoke" })}>Revoke key</button></div>}
    {confirmation?.key.id === k.id && <div className="notice key-confirm"><p>{confirmation.action === "rotate" ? "Rotate" : "Revoke"} “{k.name}”? Its current secret stops working immediately.{confirmation.action === "rotate" && " The replacement keeps its permissions and expiration. For an overlap period, create a separate key, update the integration, then revoke the old key."}</p><div className="site-actions"><button className="primary" disabled={busy} onClick={() => void change(`/api-keys/${k.id}/${confirmation.action}`, { version: k.version, ...(confirmation.action === "rotate" ? { key: { name: k.name, scopes: k.scopes, expires_at: k.expires_at } } : {}) })}>Confirm {confirmation.action}</button><button className="secondary" disabled={busy} onClick={() => setConfirmation(null)}>Keep current key</button></div></div>}
  </article>;

  return <div className="conversion-layout">
    {error && <p className="error" role="alert">{error}</p>}
    {notice && <p className="notice" role="status">{notice}</p>}
    {secret && <section className="card site-panel"><h2>Save your new API key</h2><p>This secret cannot be retrieved again. Store it in your password or secret manager, never in agent chat. Leaving this page clears it from the screen.</p><textarea aria-label="New API key" className="site-code" readOnly value={secret} rows={3} /><div className="site-actions"><button className="secondary" onClick={() => void navigator.clipboard?.writeText(secret)}>Copy</button><button className="secondary" onClick={() => { setSecret(""); setNotice(""); }}>I saved the key</button></div></section>}

    <section className="card site-panel">
      <div className="section-heading"><h2>Workspace keys</h2><div className="site-actions"><button className="secondary" disabled={busy} onClick={() => void refresh().catch(e => setError(e.message))}>Refresh keys</button>{data && !open && <button className="primary" disabled={!!secret} onClick={() => setOpen(true)}>New key</button>}</div></div>
      {!data ? <p>Loading keys…</p> : !active.length ? <p className="muted">No active keys.</p> : <div className="key-list">{active.map(row)}</div>}
      {inactive.length > 0 && <details className="key-inactive"><summary>Revoked and expired ({inactive.length})</summary><div className="key-list">{inactive.map(row)}</div></details>}
    </section>

    {open && <section className="card site-panel">
      <div className="section-heading"><h2>Create an API key</h2><button className="secondary" onClick={() => setOpen(false)}>Cancel</button></div>
      {setup && <p className="notice">Temporary setup key: read + website/DNI setup, expiring in seven days. Add number registration only if your agent will register owned numbers. Save the key privately with the CLI; never paste it into agent chat. Revoke it here when setup is finished.</p>}
      <div className="key-presets"><span className="muted">Start from</span>{presets.map(p => <button type="button" key={p.label} className="chip" onClick={() => preset(p)}>{p.label}</button>)}</div>
      <form className="site-form" onSubmit={create}>
        <div className="key-fields">
          <div className="form-field"><label htmlFor="key-name">Key name</label><input id="key-name" required maxLength={80} value={name} onChange={e => setName(e.target.value)} placeholder="CRM integration" /></div>
          <div className="form-field"><label htmlFor="key-expiry">Expires</label><select id="key-expiry" value={expiry} onChange={e => setExpiry(e.target.value as Expiry)}><option value="24h">In 24 hours</option><option value="7d">In 7 days</option><option value="30d">In 30 days</option><option value="90d">In 90 days</option><option value="never">Never</option><option value="custom">Custom…</option></select></div>
          {expiry === "custom" && <div className="form-field"><label htmlFor="key-expiration">Expires at (UTC)</label><input id="key-expiration" type="datetime-local" step="1" required value={custom} onChange={e => setCustom(e.target.value)} /></div>}
        </div>
        <fieldset className="key-group"><legend>Write permissions</legend><p className="muted">Read access is always included, and covers customer contact details and notes. Grant only what the integration needs.</p>
          {writeScopes.map(([scope, title]) => <label className="site-toggle" key={scope}><input type="checkbox" checked={scopes.includes(scope)} onChange={e => toggle(scope, e.target.checked)} />{title}</label>)}
        </fieldset>
        <fieldset className="key-group key-group-risk"><legend>High-risk permissions</legend><p className="muted">These default to a 24-hour key. Revoke it as soon as the task is done.</p>
          <label className="site-toggle"><input type="checkbox" checked={scopes.includes("numbers:purchase")} onChange={e => toggle("numbers:purchase", e.target.checked)} />Search, buy, and release phone numbers (billable)</label>
          {scopes.includes("numbers:purchase") && <p className="notice">Every number this key buys is billed monthly until it is released: by VisiConvert for managed numbers, or by your carrier for your own SignalWire project. Purchases are capped per workspace and by your spending limit.</p>}
          <label className="site-toggle"><input type="checkbox" checked={scopes.includes("conversions:send")} onChange={e => toggle("conversions:send", e.target.checked)} />Send conversions: connect Meta, choose ad accounts, and turn delivery on or off</label>
          {scopes.includes("conversions:send") && <p className="notice">This key can save ad accounts and the Meta token, turn Google, Microsoft and Meta sending on or off, change which goals bid, and attest the advertising consent policy on your behalf; attestations are recorded in your name. Conversions already sent cannot be retracted.</p>}
        </fieldset>
        <p className="muted">Keys cannot manage other keys, billing, or provider credentials.</p>
        <div><button className="primary" disabled={busy || !!secret}>Create API key</button></div>
      </form>
    </section>}

    <details className="card site-panel"><summary>Use the API, CLI and MCP</summary>
      <p>Base URL: <code>{window.location.origin}/api/v1</code>. Send the key as a Bearer token in the Authorization header; never put it in website JavaScript or a URL.</p>
      <pre className="site-code">{`curl -H "Authorization: Bearer $VISICONVERT_API_KEY" \\\n  "${window.location.origin}/api/v1/inquiries?limit=50"\n\nvisiconvert login --url ${window.location.origin}`}</pre>
      <p>The CLI prompts for the key. MCP uses the same login and starts read-only unless you enable changes. <a href="/api/v1/openapi.json" target="_blank" rel="noreferrer">OpenAPI specification</a></p>
      <p className="muted">Up to 200 keys, active first; up to 50 active at once. Each key permits 120 requests per minute. Hosted keys require active workspace access and are invalidated by an owner password reset. Revocation and rotation reject new requests immediately; requests already in progress may finish.</p>
    </details>
    <details className="card site-panel"><summary>Key management history (latest 100)</summary><ul className="inquiry-history">{data?.history.map(h => <li key={h.id}>{h.kind} · {h.actor} · {new Date(h.created_at).toLocaleString()}</li>)}</ul></details>
  </div>;
}
