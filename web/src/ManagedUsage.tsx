import { useCallback, useEffect, useState } from "react";

type Request = <T>(path: string, body?: unknown, method?: string) => Promise<T>;
type Payment = { status: string; payment_required: boolean; amount_remaining_cents: number; next_payment_attempt_at?: string };
type Period = {
  id: number; kind?: string; payment?: Payment; final_invoice?: boolean; starts_at: string; ends_at: string; status: string; invoice_status?: string;
  observed_at?: string; estimate_unavailable?: string; reserved_cents?: number;
  estimate?: { gross_cents: number; credit_cents: number; amount_cents: number };
  entries?: { kind: string; amount_cents: number }[];
};
type Usage = {
  enrolled: boolean; spending_blocked?: boolean; subscription_status?: string; payment_issues?: Payment[]; currency: string; monthly_limit_cents: number | null;
  pricing: { number_month_cents: number; minute_cents: number; credit_cents: number } | null;
  service?: { enforced: boolean; synced: boolean; grace_ends_at?: string; policy: { state?: string; reason?: string; release_at?: number; valid_until?: number } };
  periods: Period[];
};
const dollars = (cents: number) => new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(cents / 100);
const date = (value: string) => new Date(value).toLocaleDateString(undefined, { timeZone: "UTC", year: "numeric", month: "short", day: "numeric" });

export function ManagedUsage({ workspaceId, request }: { workspaceId: string; request: Request }) {
  const [usage, setUsage] = useState<Usage | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const reload = useCallback(async () => {
    setBusy(true); setError("");
    try { setUsage(await request<Usage>(`/workspaces/${workspaceId}/billing/usage`)); }
    catch { setError("Usage billing is temporarily unavailable. Please try again."); }
    finally { setBusy(false); }
  }, [workspaceId, request]);
  useEffect(() => { setUsage(null); void reload(); }, [reload]);
  if (!usage && !error) return <p role="status">Loading usage…</p>;
  if (usage && !usage.enrolled && !error) return null;
  return <section className="managed-usage" aria-labelledby="managed-usage-title">
    <h2 id="managed-usage-title">Managed numbers and calls</h2>
    {error && <p role="alert" className="error">{error}</p>}
    {usage?.pricing && <p>{dollars(usage.pricing.number_month_cents)} per number/month · {dollars(usage.pricing.minute_cents)} per minute. Includes {dollars(usage.pricing.credit_cents)} usage credit each paid billing period; unused credit does not carry forward.</p>}
    {usage?.monthly_limit_cents != null && <p>Configured usage budget: {dollars(usage.monthly_limit_cents)} before credit. Reservations below are pending spending, not charges.</p>}
    {usage?.subscription_status === "canceled" && <p role="status">Subscription ended. New managed-number purchases are blocked. Outstanding usage and number rental during the hold still need to be settled.</p>}
    {usage?.service?.enforced && <div role="status">
      {!usage.service.synced ? <p>Updating call service status. Refresh to confirm the change has reached your workspace.</p> :
        usage.service.policy.valid_until && usage.service.policy.valid_until * 1000 <= Date.now() ? <p>Call authorization has expired. Forwarding is paused while billing status is refreshed; this alone does not release numbers.</p> :
        usage.service.policy.state === "grace" ? <p>Calls continue during your payment grace period{usage.service.grace_ends_at ? ` through ${date(usage.service.grace_ends_at)}` : ""}. Resolve payment before then to avoid a forwarding pause.</p> :
        usage.service.policy.state === "suspended" ? <p>Managed call forwarding is paused. {usage.service.policy.release_at ? `Numbers are held until ${date(new Date(usage.service.policy.release_at * 1000).toISOString())}, then scheduled for release. Restore billing before release begins to keep them.` : "Contact support to review billing; no automatic number release is scheduled."}</p> :
        usage.service.policy.state === "active" ? <p>Managed call forwarding is active.</p> : <p>Waiting for confirmed call service status.</p>}
    </div>}
    {!!usage?.payment_issues?.length && <div role="alert">
      <p>Billing needs attention. New managed-number purchases are blocked. Open Manage billing to review invoices and update your payment method.</p>
      {usage.payment_issues.map((payment, index) => <p key={index}>
        {payment.status === "void" || payment.status === "uncollectible" ? "An invoice needs support review." : `${dollars(payment.amount_remaining_cents)} remains unpaid.`}
        {payment.next_payment_attempt_at && ` Next payment attempt: ${date(payment.next_payment_attempt_at)}.`}
      </p>)}
    </div>}
    {usage?.spending_blocked && usage.subscription_status !== "canceled" && !usage.payment_issues?.length && <p>New managed-number purchases are blocked until billing eligibility is confirmed.</p>}
    {usage?.enrolled && usage.periods.length === 0 && <p>No billing periods yet. A period opens after the subscription invoice is paid.</p>}
    {usage?.periods.map(period => {
      const sealed = period.entries && period.entries.length > 0;
      const gross = sealed ? period.entries!.filter(e => e.kind !== "credit").reduce((sum, e) => sum + e.amount_cents, 0) : period.estimate?.gross_cents;
      const credit = sealed ? -period.entries!.filter(e => e.kind === "credit").reduce((sum, e) => sum + e.amount_cents, 0) : period.estimate?.credit_cents;
      const net = gross != null && credit != null ? gross - credit : undefined;
      const status = period.status === "review" || period.invoice_status === "review" ? "Needs review — the sealed amount has not been changed" :
        period.invoice_status === "posted" ? net === 0 ? "No additional usage charge" :
        period.payment?.status === "paid" ? "Invoice paid" :
        period.payment?.payment_required ? "Invoice needs attention — open Manage billing" :
        period.payment?.status === "open" ? "Invoiced — awaiting payment" : "Added to invoice — payment not yet confirmed" :
        period.invoice_status === "submitting" ? "Checking invoice submission" : sealed ? "Awaiting invoice" : "Estimated usage — period still open";
      return <article className="managed-usage-period" key={period.id}>
        <h3>{date(period.starts_at)} – {date(period.ends_at)} (UTC)</h3>
        <p>{status}</p>
        {period.kind === "tail" && <p>Usage after the last paid period, including number release holds. No additional monthly credit applies.</p>}
        {period.final_invoice && <p>Final usage invoice for this period.</p>}
        {gross != null && credit != null && net != null ? <dl>
          <div><dt>Usage</dt><dd>{dollars(gross)}</dd></div>
          <div><dt>Usage credit</dt><dd>{credit ? `−${dollars(Math.abs(credit))}` : dollars(0)}</dd></div>
          <div><dt>{sealed ? "Additional usage" : "Estimated additional usage"}</dt><dd>{dollars(net)}</dd></div>
        </dl> : <p>Waiting for verified usage quantities.</p>}
        {period.observed_at && <p className="muted">Usage observed {date(period.observed_at)}.</p>}
        {!!period.reserved_cents && <p>Pending reservations: {dollars(period.reserved_cents)}</p>}
      </article>;
    })}
    <button className="secondary" disabled={busy} onClick={() => void reload()}>{busy ? "Refreshing usage…" : "Refresh usage"}</button>
  </section>;
}
