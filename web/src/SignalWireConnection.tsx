import { useEffect, useState, type FormEvent } from "react";
import type { Client, SignalWireConnection as Connection, TwilioInventory } from "./api";

export function SignalWireConnection({ api, connection, refresh }: { api: Client; connection: Connection; refresh: () => Promise<void> }) {
  const [space, setSpace] = useState(connection.space_host || "");
  const [project, setProject] = useState(connection.project_id || "");
  const [token, setToken] = useState("");
  const [signing, setSigning] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [inventory, setInventory] = useState<TwilioInventory | null>(null);
  useEffect(() => { setSpace(connection.space_host || ""); setProject(connection.project_id || ""); setInventory(null); }, [connection.revision, connection.space_host, connection.project_id]);
  async function save(e: FormEvent) {
    e.preventDefault(); setBusy(true); setError(""); setNotice("");
    try {
      await api("/connections/signalwire", { space_host: space.trim().toLowerCase(), project_id: project.trim(), api_token: token.trim(), signing_key: signing.trim(), revision: connection.revision }, "PUT");
      setToken(""); setSigning(""); await refresh();
      setNotice("SignalWire account and number access verified. Configure a number and place a test call to verify the signing key and routing.");
    } catch (e) { setError((e as Error).message); }
    finally { setToken(""); setSigning(""); setBusy(false); }
  }
  return <section className="card connection" aria-label="SignalWire connection">
    <div className="card-header"><div><h2>SignalWire → your business phone</h2><p>Use an existing project and forward calls to your current phone system.</p></div><span className={`badge ${connection.connected ? "qualified" : "unreviewed"}`}>{connection.connected ? "Account verified" : "Not connected"}</span></div>
    <div className="connection-body">
      {connection.setup_message && <p className="notice">{connection.setup_message}</p>}
      <form onSubmit={save}>
        <label htmlFor="signalwire-space">SignalWire Space hostname</label><input id="signalwire-space" value={space} onChange={e => setSpace(e.target.value)} placeholder="your-space.signalwire.com" autoComplete="off" required maxLength={78} disabled={busy || !connection.setup_available} />
        <label htmlFor="signalwire-project">SignalWire Project ID</label><input id="signalwire-project" value={project} onChange={e => setProject(e.target.value)} autoComplete="off" required maxLength={36} disabled={busy || !connection.setup_available} />
        <label htmlFor="signalwire-token">SignalWire API token</label><input id="signalwire-token" type="password" value={token} onChange={e => setToken(e.target.value)} autoComplete="off" required minLength={16} maxLength={512} disabled={busy || !connection.setup_available} />
        <label htmlFor="signalwire-signing">SignalWire webhook signing key</label><input id="signalwire-signing" type="password" value={signing} onChange={e => setSigning(e.target.value)} autoComplete="off" required minLength={16} maxLength={512} disabled={busy || !connection.setup_available} />
        <p className="small muted">Use a token with account and number read access. The project’s webhook signing key is separate from its API token. Both secrets are encrypted and never returned. Use a separate project for each business.</p>
        <button className="primary" disabled={busy || !connection.setup_available}>{busy ? "Checking SignalWire…" : connection.connected ? "Verify and update SignalWire" : "Verify and connect SignalWire"}</button>
      </form>
      {error && <p className="error" role="alert">{error}</p>}{notice && <p className="success" role="status">{notice}</p>}
      {connection.connected && <>
        <p className="notice">{connection.last_webhook_at ? `Signed callback received ${new Date(connection.last_webhook_at).toLocaleString()}. Confirm that your destination answered the test call.` : "Waiting for the first signed callback. Account verification alone does not prove call routing works."}</p>
        <button className="secondary" disabled={busy} onClick={async () => { setBusy(true); setError(""); try { setInventory(await api<TwilioInventory>("/connections/signalwire/numbers")); } catch(e) { setError((e as Error).message); } finally { setBusy(false); } }}>Check SignalWire numbers</button>
        {inventory && <div role="status">{inventory.numbers.length ? <ul>{inventory.numbers.map(n => <li key={n.sid}>{n.phone_number} · {n.friendly_name}</li>)}</ul> : <p>No supported US voice numbers found.</p>}{inventory.has_more && <p>Showing the first page. Enter another owned number in Tracking numbers to verify it directly.</p>}</div>}
        <ol><li>Register an existing number with SignalWire selected in Tracking numbers.</li><li>In SignalWire, configure that number for cXML webhooks. Set incoming voice and call status callbacks to HTTP POST using the URLs below.</li><li>Call the number, answer at the destination, and check the Lead inbox. Configure independent carrier fallback routing before production use.</li></ol>
        <p>Incoming voice webhook</p><code className="code-block">{connection.voice_url}</code><p>Call status callback</p><code className="code-block">{connection.status_url}</code>
        <p className="small muted">Connecting does not change routing or buy numbers. Replacing an existing SWML or AI handler changes how that number answers; save its settings before moving a test number. Recording is off. SMS and number purchases are not included. Add registered numbers to DNI pools in Sites.</p>
        {connection.can_disconnect ? <button className="text-button" disabled={busy} onClick={async () => { setBusy(true); setError(""); try { await api("/connections/signalwire", { revision: connection.revision }, "DELETE"); await refresh(); setNotice("SignalWire disconnected. Saved credentials removed."); } catch(e) { setError((e as Error).message); } finally { setBusy(false); } }}>Disconnect unused SignalWire project</button> : <p className="small muted">Registered numbers require this project. You can rotate credentials while keeping the same Space and Project ID.</p>}
      </>}
    </div>
  </section>;
}
