import { useEffect, useState, type FormEvent } from "react";
import { type Client } from "./api";
type Key = { id: string; name: string; prefix: string; scopes: string[]; expires_at: string | null; revoked_at: string | null; last_used_at: string | null; version: number };
type Data = { keys: Key[]; history: { id: number; kind: string; actor: string; created_at: string }[] };
const writeScopes = [["leads:write", "Create interactions and review inquiries"], ["bookings:write", "Record, revise, and cancel bookings"], ["numbers:write", "Register existing Twilio numbers"]];
export function ApiKeys({ api }: { api: Client }) {
  const [data, setData] = useState<Data | null>(null);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState(["read"]);
  const [expires, setExpires] = useState("");
  const [secret, setSecret] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<{ key: Key; action: "rotate" | "revoke" } | null>(null);
  async function refresh() { setData(await api<Data>("/api-keys")); }
  useEffect(() => { let active = true; api<Data>("/api-keys").then(d => { if (active) setData(d); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [api]);
  async function change(path: string, body: unknown) {
    setBusy(true); setError(""); setNotice(""); setSecret("");
    try { const result = await api<{ secret?: string }>(path, body); setSecret(result.secret ?? ""); setConfirmation(null); await refresh(); setNotice(result.secret ? "Save this key now. Its secret is shown only once." : "Key revoked. New requests using it are rejected."); }
    catch (e) { setError((e as Error).message); }
    finally { setBusy(false); }
  }
  function create(e: FormEvent) { e.preventDefault(); void change("/api-keys", { name, scopes, expires_at: expires ? new Date(`${expires}Z`).toISOString() : null }); }
  return <div className="conversion-layout">
    {error && <p className="error" role="alert">{error}</p>}
    {notice && <p className="notice" role="status">{notice}</p>}
    {secret && <section className="card site-panel"><h2>Save your new API key</h2><p>This secret cannot be retrieved again. Store it in your password or secret manager. Leaving this page clears it from the screen.</p><textarea aria-label="New API key" className="site-code" readOnly value={secret} rows={3} /><button className="secondary" onClick={() => { setSecret(""); setNotice(""); }}>I saved the key</button></section>}
    <section className="card site-panel"><h2>Create an API key</h2><p className="muted">One key belongs to this workspace. Read access includes customer contact information and notes. Grant only the write permissions the integration needs. Keys cannot manage other keys, billing, or provider credentials.</p>
      <form className="site-form" onSubmit={create}>
        <div className="form-field"><label htmlFor="key-name">Key name</label><input id="key-name" required maxLength={80} value={name} onChange={e => setName(e.target.value)} placeholder="CRM integration" /></div>
        <div className="form-field"><label htmlFor="key-expiration">Expires at (UTC, optional)</label><input id="key-expiration" type="datetime-local" step="1" value={expires} onChange={e => setExpires(e.target.value)} /></div>
        <p>Read access is included. Optional write permissions:</p>
        {writeScopes.map(([scope, title]) => <label className="site-toggle" key={scope}><input type="checkbox" checked={scopes.includes(scope)} onChange={e => setScopes(e.target.checked ? [...scopes, scope] : scopes.filter(s => s !== scope))} />{title}</label>)}
        <div><button className="primary" disabled={busy || !!secret}>Create API key</button></div>
      </form>
    </section>
    <section className="card site-panel"><div className="section-heading"><h2>Workspace keys</h2><button className="secondary" disabled={busy} onClick={() => void refresh().catch(e => setError(e.message))}>Refresh keys</button></div>
      <p className="muted">Up to 200 keys, active first; up to 50 active at once. Each key permits 120 requests per minute. Hosted keys require active workspace access and are invalidated by an owner password reset. Revocation and rotation reject new requests immediately; requests already in progress may finish.</p>
      {!data ? <p>Loading keys…</p> : !data.keys.length ? <p>No API keys yet.</p> : data.keys.map(k => <article className="conversion-job" key={k.id} aria-label={`API key ${k.name}`}>
        <strong>{k.name}</strong><code>{k.prefix}…</code><span>{k.revoked_at ? "Revoked" : k.expires_at && new Date(k.expires_at) <= new Date() ? "Expired" : "Active"}</span>
        <small>Permissions: {k.scopes.join(", ")}</small><small>Expires: {k.expires_at ? new Date(k.expires_at).toLocaleString() : "No expiration"} · Last used: {k.last_used_at ? new Date(k.last_used_at).toLocaleString() : "Never"}</small>
        {!k.revoked_at && <div className="site-actions"><button className="secondary" disabled={busy || !!secret} onClick={() => setConfirmation({ key: k, action: "rotate" })}>Rotate key</button><button className="secondary" disabled={busy} onClick={() => setConfirmation({ key: k, action: "revoke" })}>Revoke key</button></div>}
      </article>)}
      {confirmation && <div className="notice"><p>{confirmation.action === "rotate" ? "Rotate" : "Revoke"} “{confirmation.key.name}”? Its current secret will stop working immediately.{confirmation.action === "rotate" && " The replacement keeps its permissions and expiration. For an overlap period, create a separate key, update the integration, then revoke the old key."}</p><div className="site-actions"><button className="primary" disabled={busy} onClick={() => void change(`/api-keys/${confirmation.key.id}/${confirmation.action}`, { version: confirmation.key.version, ...(confirmation.action === "rotate" ? { key: { name: confirmation.key.name, scopes: confirmation.key.scopes, expires_at: confirmation.key.expires_at } } : {}) })}>Confirm {confirmation.action}</button><button className="secondary" disabled={busy} onClick={() => setConfirmation(null)}>Keep current key</button></div></div>}
    </section>
    <section className="card site-panel"><h2>Use the public API</h2><p>Base URL: <code>{window.location.origin}/api/v1</code></p><p>Send the key in the Authorization header as a Bearer token. Never put it in website JavaScript or a URL.</p><pre className="site-code">{`curl -H "Authorization: Bearer $VISICONVERT_API_KEY" \\\n  "${window.location.origin}/api/v1/inquiries?limit=50"\n\nvisiconvert login --url ${window.location.origin}`}</pre><p>The CLI prompts for this key. MCP uses the same login and starts read-only unless you enable changes.</p><a href="/api/v1/openapi.json" target="_blank" rel="noreferrer">OpenAPI specification</a></section>
    <details className="card site-panel"><summary>Key management history (latest 100)</summary><ul className="inquiry-history">{data?.history.map(h => <li key={h.id}>{h.kind} · {h.actor} · {new Date(h.created_at).toLocaleString()}</li>)}</ul></details>
  </div>;
}
