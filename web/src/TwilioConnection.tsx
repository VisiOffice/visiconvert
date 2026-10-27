import { useEffect, useState, type FormEvent } from "react";
import type { Client, TwilioConnection as Connection, TwilioInventory } from "./api";

export function TwilioConnection({ api, connection, refresh }: {
  api: Client; connection: Connection; refresh: () => Promise<void>;
}) {
  const [account, setAccount] = useState(connection.account_sid || "");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [inventory, setInventory] = useState<TwilioInventory | null>(null);
  useEffect(() => { setAccount(connection.account_sid || ""); setInventory(null); }, [connection.account_sid, connection.revision]);

  async function save(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError(""); setNotice("");
    try {
      await api("/connections/twilio", { account_sid: account.trim(), auth_token: token.trim(), revision: connection.revision }, "PUT");
      setToken("");
      await refresh();
      setNotice("Twilio account verified and connected. Register a number, then configure its callbacks below.");
    } catch (error) { setError((error as Error).message); }
    finally { setToken(""); setBusy(false); }
  }

  return <section className="card connection">
    <div className="card-header">
      <div><h2>Twilio → your business phone</h2><p>Connect your account. Answer calls in Quo, RingCentral, or your existing phone system.</p></div>
      <span className={`badge ${connection.connected ? "qualified" : "unreviewed"}`}>
        {connection.mode === "workspace" ? "Account verified" : connection.mode === "environment" ? "Environment configured" : "Not connected"}
      </span>
    </div>
    <div className="connection-body">
      {connection.setup_message && <p className="notice">{connection.setup_message}</p>}
      {connection.mode === "environment" ? <p>Account: <code>{connection.account_sid}</code>. Existing callbacks remain active.</p> :
        <form onSubmit={save}>
          <label htmlFor="twilio-account">Twilio Account SID</label>
          <input id="twilio-account" value={account} onChange={e => setAccount(e.target.value)} placeholder="AC…" autoComplete="off" required pattern="AC[0-9a-fA-F]{32}" maxLength={34} disabled={busy || !connection.setup_available} />
          <label htmlFor="twilio-auth-token">{connection.connected ? "Replacement Auth Token" : "Twilio Auth Token"}</label>
          <input id="twilio-auth-token" type="password" value={token} onChange={e => setToken(e.target.value)} autoComplete="off" required pattern="[0-9a-fA-F]{32}" maxLength={32} disabled={busy || !connection.setup_available} />
          <p className="small muted">Use this account’s Auth Token from Twilio Console. It verifies incoming calls and is stored encrypted on your server. It is never shown again. A separate Twilio subaccount per business keeps provider access separate.</p>
          <button className="primary" disabled={busy || !connection.setup_available}>{busy ? "Checking Twilio…" : connection.connected ? "Verify and update credentials" : "Verify and connect Twilio"}</button>
        </form>}
      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="success" role="status">{notice}</p>}
      {connection.connected && <>
        <button className="secondary" disabled={busy} onClick={async () => {
          setBusy(true); setError("");
          try { setInventory(await api<TwilioInventory>("/connections/twilio/numbers")); }
          catch (error) { setError((error as Error).message); }
          finally { setBusy(false); }
        }}>Check available tracking numbers</button>
        {inventory && <div role="status">
          {inventory.numbers.length ? <ul>{inventory.numbers.map(n => <li key={n.sid}><strong>{n.phone_number}</strong> · {n.friendly_name}</li>)}</ul> : <p>No supported voice numbers were found in this account.</p>}
          {inventory.has_more && <p>Showing the first page of numbers. You can enter any existing number in Tracking numbers; ownership is checked when you register it.</p>}
        </div>}
        <ol>
          <li>Register a number and its forwarding destination in Tracking numbers.</li>
          <li>In Twilio, set that number’s incoming voice webhook and call status callback to HTTP POST using these URLs.</li>
          <li>Place a test call and check the Lead inbox. Configure an independent Twilio fallback before using production traffic.</li>
        </ol>
        <p>Incoming voice webhook</p><code className="code-block">{connection.voice_url}</code>
        <p>Call status callback</p><code className="code-block">{connection.status_url}</code>
        <p className="small muted">Connecting verifies account access; it does not change Twilio routing or prove that a call will connect. Recording is off. SMS and number purchases are not enabled. Configure DNI number pools in Sites.</p>
        {connection.can_disconnect && <button className="text-button" disabled={busy} onClick={async () => {
          setBusy(true); setError(""); setNotice("");
          try { await api("/connections/twilio", { revision: connection.revision }, "DELETE"); await refresh(); setNotice("Disconnected. Saved credentials were removed."); }
          catch (error) { setError((error as Error).message); }
          finally { setBusy(false); }
        }}>Disconnect unused account</button>}
        {connection.mode === "workspace" && !connection.can_disconnect && <p className="small muted">This connection has registered numbers. Credential updates must use the same Twilio account to preserve routing.</p>}
      </>}
    </div>
  </section>;
}
