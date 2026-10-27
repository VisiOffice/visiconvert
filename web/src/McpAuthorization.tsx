import { useEffect, useState, type FormEvent } from "react";
import { navigate } from "./cloud";

type Request = <T>(path: string, body?: unknown, method?: string) => Promise<T>;
type Consent = { request_token: string; client_name: string; redirect_uri: string; scopes: string[]; workspaces: { id: string; name: string; available: boolean }[] };
const descriptions: Record<string, string> = {
  read: "Read leads, contact details, bookings, reports, and tracking configuration",
  "setup:write": "Create and update websites and dynamic number pools",
  "numbers:write": "Register phone numbers you already own",
  "leads:write": "Create leads and change qualification and notes",
  "bookings:write": "Record or cancel bookings and their values",
};
export function McpAuthorization({ request, verified }: { request: Request; verified: boolean }) {
  const [data, setData] = useState<Consent | null>(null);
  const [workspace, setWorkspace] = useState("");
  const [scopes, setScopes] = useState<string[]>(["read"]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!verified) return;
    let active = true;
    request<Consent>(`/mcp/authorization${window.location.search}`).then(value => {
      if (active) { setData(value); setScopes(["read"]); }
    }).catch(e => { if (active) setError(e.message); });
    return () => { active = false; };
  }, [request, verified]);
  async function decide(decision: "approve" | "deny") {
    if (!data) return;
    setBusy(true); setError("");
    try {
      const result = await request<{ redirect_to: string }>("/mcp/authorization", { request_token: data.request_token, workspace_id: workspace, scopes, decision });
      // The server binds this to the registered redirect. Defense in depth before leaving our origin.
      const approved = new URL(data.redirect_uri), destination = new URL(result.redirect_to);
      if (approved.origin !== destination.origin || approved.pathname !== destination.pathname) throw new Error("Unexpected authorization destination.");
      window.location.assign(result.redirect_to);
    } catch (e) { setError((e as Error).message); setBusy(false); }
  }
  function submit(event: FormEvent) { event.preventDefault(); void decide("approve"); }
  return <section className="card cloud-panel" aria-label="Connect an agent">
    <span className="eyebrow">CONNECT AN AGENT</span><h1>Approve workspace access</h1>
    {!verified ? <p>Verify your email first, then restart the connection from your agent.</p> : error && !data ? <p className="error" role="alert">{error}</p> : !data ? <p role="status">Loading connection request…</p> : <>
      <p><strong>{data.client_name}</strong> is requesting access to one VisiConvert workspace.</p>
      <p>Continue only if you started this connection. The app supplied its name; check the destination before approving.</p>
      <p className="site-code">Return to: {data.redirect_uri}</p>
      <form onSubmit={submit}>
        <div className="form-field"><label htmlFor="mcp-workspace">Workspace</label><select id="mcp-workspace" required value={workspace} onChange={e => setWorkspace(e.target.value)}><option value="">Choose a workspace</option>{data.workspaces.map(w => <option key={w.id} value={w.id} disabled={!w.available}>{w.name}{w.available ? "" : " — setup or billing required"}</option>)}</select></div>
        {!data.workspaces.some(w => w.available) && <p>No ready workspaces are available. <a href="/workspaces">Manage your workspaces</a>, then restart this connection.</p>}
        <fieldset><legend>Permissions</legend>{data.scopes.map(scope => <label className="site-toggle" key={scope}><input type="checkbox" checked={scopes.includes(scope)} disabled={scope === "read"} onChange={e => setScopes(old => e.target.checked ? [...old, scope] : old.filter(s => s !== scope))} />{descriptions[scope] || scope}</label>)}</fieldset>
        <p>The agent cannot change your password, billing, or provider credentials. This connection expires after 90 days. Revoke it anytime in Connected agents.</p>
        {error && <p role="alert" className="error">{error}</p>}
        <div className="site-actions"><button className="primary" disabled={busy || !workspace} type="submit">Approve connection</button><button className="secondary" disabled={busy} type="button" onClick={() => void decide("deny")}>Deny</button></div>
      </form>
    </>}
  </section>;
}

type Connection = { id: string; client_name: string; workspace: string; scopes: string[]; created_at: string; expires_at: string };
export function McpConnections({ request }: { request: Request }) {
  const [items, setItems] = useState<Connection[] | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const [confirm, setConfirm] = useState("");
  useEffect(() => { let active = true; request<{ connections: Connection[] }>("/mcp_connections").then(r => { if (active) setItems(r.connections); }).catch(e => { if (active) setError(e.message); }); return () => { active = false; }; }, [request]);
  async function revoke(id: string) {
    setBusy(id); setError("");
    try { await request(`/mcp_connections/${id}`, undefined, "DELETE"); setItems(old => old?.filter(c => c.id !== id) || []); setConfirm(""); }
    catch (e) { setError((e as Error).message); }
    finally { setBusy(""); }
  }
  return <section className="card cloud-panel"><h1>Connected agents</h1><p>Each connection can access only its approved workspace and permissions.</p>
    <p>Remote MCP URL: <code>{window.location.origin}/mcp</code></p>
    {error && <p role="alert" className="error">{error}</p>}
    {!items ? <p>Loading connections…</p> : !items.length ? <p>No active agent connections.</p> : items.map(c => <article className="card site-panel" key={c.id}><h2>{c.client_name}</h2><p>{c.workspace}</p><ul>{c.scopes.map(s => <li key={s}>{descriptions[s] || s}</li>)}</ul><p>Expires {new Date(c.expires_at).toLocaleDateString()}</p>{confirm === c.id ? <div><p>Revoke this connection? The agent will need your approval to reconnect.</p><button className="primary" disabled={!!busy} onClick={() => void revoke(c.id)}>Confirm revoke</button><button className="secondary" onClick={() => setConfirm("")}>Keep connection</button></div> : <button className="secondary" onClick={() => setConfirm(c.id)}>Revoke connection</button>}</article>)}
    <button className="secondary" onClick={() => navigate("/workspaces")}>Back to workspaces</button>
  </section>;
}
