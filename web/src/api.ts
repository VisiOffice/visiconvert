export type Lead = {
  id: string;
  name: string;
  contact: string;
  channel: string;
  source: string;
  campaign: string;
  qualification: string;
  notes: string;
  version: number;
  created_at: string;
  is_demo: boolean;
  call_status: string | null;
  duration_seconds: number | null;
  tracking_number: string | null;
  destination: string | null;
  booked_minor: number;
};
export type TrackingNumber = {
  id: string;
  label: string;
  phone_number: string;
  destination: string;
  source: string;
  campaign: string;
  is_demo: boolean;
};
export type Summary = {
  source: string;
  leads: number;
  qualified: number;
  bookings: number;
  booked_minor: number;
};
export type Bootstrap = {
  hosted: boolean;
  name: string;
  twilio_configured: boolean;
  voice_url: string;
  status_url: string;
  mode: string;
};
export type Outcome = {
  external_id: string;
  amount_minor: number;
  currency: string;
  status: string;
  revision: number;
};
export type TwilioConnection = {
  mode: "workspace" | "environment" | "disconnected";
  connected: boolean;
  account_sid: string | null;
  revision: number;
  setup_available: boolean;
  setup_message: string | null;
  can_disconnect: boolean;
  voice_url: string;
  status_url: string;
};
export type TwilioInventory = {
  numbers: { sid: string; phone_number: string; friendly_name: string }[];
  has_more: boolean;
};
export function client(key: string) {
  return async function request<T>(
    path: string,
    body?: unknown,
    method = body ? "POST" : "GET",
  ): Promise<T> {
    const response = await fetch(`/api${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${key}`,
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!response.ok) {
      const value = await response
        .json()
        .catch(() => ({ error: `Request failed (${response.status})` }));
      throw new Error(value.error || "Request failed");
    }
    return response.json();
  };
}
export type Client = ReturnType<typeof client>;
export const money = (amount: number) =>
  new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2,
  }).format(amount / 100);
export const label = (s: string) => s.replaceAll("_", " ").replaceAll("-", " ");
