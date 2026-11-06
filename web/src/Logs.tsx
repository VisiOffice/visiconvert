import { useEffect, useRef, useState, type ComponentType } from "react";
import { money, type Client } from "./api";
import { Activity, Check, ChevronDown, Code2, FileText, Loader2, Phone, RefreshCw, ScrollText, type IconProps } from "./icons";

type Kind = "call" | "form" | "conversion" | "webhook" | "api";
type Outcome = "success" | "warning" | "failure" | "pending" | "neutral";
export type ActivityItem = {
  id: string | number; source: string; kind: Kind; occurred_at: string; summary: string;
  status: string; outcome: Outcome; actor: string | null; lead_id: string | null;
  inquiry_id: string | null; is_demo: boolean; detail: Record<string, unknown>;
};
type Page = { data: ActivityItem[]; next_cursor: string | null };

const kinds: { value: Kind | "all"; label: string; empty: string }[] = [
  { value: "all", label: "All", empty: "activity" },
  { value: "call", label: "Calls", empty: "calls" },
  { value: "form", label: "Forms", empty: "form submissions" },
  { value: "conversion", label: "Conversions", empty: "conversion events or deliveries" },
  { value: "webhook", label: "Webhooks", empty: "webhook deliveries" },
  { value: "api", label: "API", empty: "API, agent or settings changes" },
];
const ranges = [
  { value: "24h", label: "24h", name: "the last 24 hours", hours: 24 },
  { value: "7d", label: "7d", name: "the last 7 days", hours: 24 * 7 },
  { value: "30d", label: "30d", name: "the last 30 days", hours: 24 * 30 },
];
const kindMeta: Record<Kind, { label: string; icon: ComponentType<IconProps> }> = {
  call: { label: "Call", icon: Phone },
  form: { label: "Form", icon: FileText },
  conversion: { label: "Conversion", icon: Check },
  webhook: { label: "Webhook", icon: Activity },
  api: { label: "API", icon: Code2 },
};
const detailLabels: Record<string, string> = {
  contact: "Contact", campaign: "Campaign", tracking_number: "Tracking number", forwarded_to: "Forwarded to",
  duration_seconds: "Duration", provider: "Carrier", site: "Website", action: "Action", external_id: "External ID",
  revision: "Revision", value_minor: "Value", matched_by: "Matched by", sendable: "Sendable", reason: "Reason",
  destination: "Destination", goal_name: "Goal", attempts: "Attempts", diagnostic: "Diagnostic", event_type: "Event",
  endpoint: "Endpoint", endpoint_id: "Endpoint ID", endpoint_description: "Description", state: "State",
  http_status: "HTTP status", error: "Error", last_attempt_at: "Last attempt", event: "Change", key_name: "API key",
  endpoint_host: "Endpoint",
};
function format(key: string, value: unknown) {
  if (key === "value_minor" && typeof value === "number") return money(value);
  if (key === "duration_seconds" && typeof value === "number") return value >= 60 ? `${Math.floor(value / 60)}m ${value % 60}s` : `${value}s`;
  if (key.endsWith("_at") && typeof value === "string") return new Date(value).toLocaleString();
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (key === "destination" && value === "google") return "Google Ads";
  if (key === "destination" && value === "microsoft") return "Microsoft Advertising";
  return String(value).replaceAll("_", " ");
}
const statusLabel = (value: string) => { const text = value.replaceAll(/[_-]/g, " "); return text.charAt(0).toUpperCase() + text.slice(1); };
const time = (value: string) => new Date(value).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
function day(value: string) {
  const date = new Date(value), today = new Date();
  const yesterday = new Date(); yesterday.setDate(today.getDate() - 1);
  const label = date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  if (date.toDateString() === today.toDateString()) return `Today · ${label}`;
  if (date.toDateString() === yesterday.toDateString()) return `Yesterday · ${label}`;
  return label;
}

export function Logs({ api, onOpenLead }: { api: Client; onOpenLead: (inquiryId: string) => Promise<void> }) {
  const [kind, setKind] = useState<Kind | "all">("all");
  const [range, setRange] = useState("7d");
  const [items, setItems] = useState<ActivityItem[] | null>(null);
  const [next, setNext] = useState<string | null>(null);
  const [since, setSince] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const request = useRef(0);
  const path = (from: string, cursor?: string | null) => {
    const query = new URLSearchParams({ limit: "50", since: from });
    if (kind !== "all") query.set("kind", kind);
    if (cursor) query.set("cursor", cursor);
    return `/activity?${query}`;
  };
  useEffect(() => {
    const id = ++request.current;
    const hours = ranges.find(r => r.value === range)?.hours || 24 * 7;
    const from = new Date(Date.now() - hours * 3_600_000).toISOString();
    setBusy(true); setError(""); setOpen(null);
    api<Page>(path(from))
      .then(page => { if (id !== request.current) return; setItems(page.data); setNext(page.next_cursor); setSince(from); })
      .catch(e => { if (id === request.current) { setItems(current => current || []); setError(e.message); } })
      .finally(() => { if (id === request.current) setBusy(false); });
    // A renewed hosted token replaces `api`; keep the loaded page rather than refetching.
  }, [kind, range, reload]);
  async function more() {
    if (!next) return;
    const id = ++request.current;
    setBusy(true); setError("");
    try {
      const page = await api<Page>(path(since, next));
      if (id !== request.current) return;
      setItems(current => [...(current || []), ...page.data]); setNext(page.next_cursor);
    } catch (e) { if (id === request.current) setError((e as Error).message); }
    finally { if (id === request.current) setBusy(false); }
  }
  const rangeName = ranges.find(r => r.value === range)?.name || "this period";
  const kindName = kinds.find(k => k.value === kind)?.empty || "activity";
  let lastDay = "";
  return <section className="card logs" aria-label="Activity log">
    <div className="card-header logs-header">
      <div><h2>Activity</h2><p>Calls, forms, conversions, webhook deliveries and changes made in the app, by API keys, or by agents.</p></div>
      <button className="secondary icon-text" disabled={busy} onClick={() => setReload(n => n + 1)}><RefreshCw size={15} />Refresh log</button>
    </div>
    <div className="logs-filters">
      <div className="chip-group" role="radiogroup" aria-label="Show">
        {kinds.map(k => <button key={k.value} role="radio" aria-checked={kind === k.value} className={`filter-chip ${kind === k.value ? "on" : ""}`} onClick={() => setKind(k.value)}>{k.label}</button>)}
      </div>
      <div className="segmented" role="radiogroup" aria-label="Time range">
        {ranges.map(r => <button key={r.value} role="radio" aria-checked={range === r.value} aria-label={`Last ${r.label}`} className={range === r.value ? "on" : ""} onClick={() => setRange(r.value)}>{r.label}</button>)}
      </div>
    </div>
    {error && <p className="error logs-error" role="alert">{error}</p>}
    {items === null ? <div className="logs-loading" aria-live="polite"><Loader2 className="spin" size={18} />Loading activity…</div>
      : !items.length ? <div className="empty">
          <span className="empty-icon"><ScrollText size={22} /></span>
          <h3>No {kindName} in {rangeName}</h3>
          <p>{kind === "webhook" ? "Deliveries appear once an endpoint in Webhooks receives an event." : kind === "api" ? "Settings, API key and webhook changes appear here with who made them." : kind === "conversion" ? "Pipeline events posted by your systems and their Google and Microsoft deliveries appear here." : "New calls, forms, deliveries and changes appear here as they happen."}</p>
          {range !== "30d" && <button className="secondary" onClick={() => setRange("30d")}>Show the last 30 days</button>}
        </div>
      : <ol className="timeline">
          {items.map(item => {
            const key = `${item.source}:${item.id}`;
            const heading = day(item.occurred_at);
            const showDay = heading !== lastDay; lastDay = heading;
            const meta = kindMeta[item.kind] || kindMeta.api;
            const details = Object.entries(item.detail || {}).filter(([, v]) => v !== null && v !== "" && v !== undefined);
            const expanded = open === key;
            return <li key={key} className={expanded ? "open" : ""}>
              {showDay && <h3 className="timeline-day">{heading}</h3>}
              <button className="timeline-row" aria-expanded={expanded} aria-controls={`activity-${key}`} onClick={() => setOpen(expanded ? null : key)}>
                <time dateTime={item.occurred_at}>{time(item.occurred_at)}</time>
                <span className={`kind-badge kind-${item.kind}`}><meta.icon size={13} />{meta.label}</span>
                <span className="timeline-summary">{item.summary}{item.is_demo && <em>DEMO</em>}{item.actor && <small>by {item.actor}</small>}</span>
                <span className={`status-pill status-${item.outcome}`}><i />{statusLabel(item.status)}</span>
                <ChevronDown className="timeline-chevron" size={16} />
              </button>
              {expanded && <div className="timeline-detail" id={`activity-${key}`}>
                <dl>
                  <div><dt>When</dt><dd>{new Date(item.occurred_at).toLocaleString()}</dd></div>
                  {item.actor && <div><dt>Changed by</dt><dd>{item.actor}</dd></div>}
                  {details.map(([k, v]) => <div key={k}><dt>{detailLabels[k] || k.replaceAll("_", " ")}</dt><dd>{format(k, v)}</dd></div>)}
                </dl>
                {item.inquiry_id && <button className="secondary" onClick={() => void onOpenLead(item.inquiry_id!).catch(e => setError(e.message))}>Open lead</button>}
              </div>}
            </li>;
          })}
        </ol>}
    {items && items.length > 0 && <div className="logs-footer">
      <span>{items.length} {items.length === 1 ? "entry" : "entries"} · {rangeName}</span>
      {next ? <button className="secondary" disabled={busy} onClick={() => void more()}>{busy ? "Loading…" : "Load more"}</button> : <span>End of log</span>}
    </div>}
  </section>;
}
