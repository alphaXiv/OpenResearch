import type { OpenResearchAccount as Account } from "../api";
import { OpenResearchAccountError } from "../api";
import { m } from "../paraglide/messages.js";

export function accountErrorMessage(error: Error | null): string | null {
  if (!error) return null;
  if (!(error instanceof OpenResearchAccountError)) return m.settings_openresearch_account_unavailable();
  return error.kind === "offline"
    ? m.account_offline_reason({ url: error.apiUrl })
    : m.account_server_reason({ url: error.apiUrl, status: error.status == null ? "" : ` (HTTP ${error.status})` });
}

export function OpenResearchAccount({ user }: { user: Account["user"] }) {
  if (!user) return <span>{m.settings_openresearch_account_unavailable()}</span>;

  const name = [user.firstName, user.lastName].filter(Boolean).join(" ");
  return <span dir="auto" className="min-w-0"><span className="block font-medium">{name}</span><span className="block overflow-x-auto whitespace-nowrap text-subtext">{user.email}</span></span>;
}
