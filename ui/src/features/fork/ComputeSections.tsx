// Colab and Kaggle compute settings, owned by the tuandang304 fork. Kept out of
// SettingsPage.tsx so upstream edits there merge cleanly; see docs/fork-maintenance.md.
import { cn } from "../../components/ui/cn";
import {
  queryClient,
} from "../../queries/client";
import { useQuery } from "@tanstack/react-query";

import {
  getColabSettingsQuery,
  getKaggleSettingsQuery,
} from "../../queries/settings";

import {
  Plus,
  RefreshCw,
  Square,
  Trash2,
} from "lucide-react";
import { useState } from "react";
import {
  fmtNumber,
  type ColabSettings,
  type ColabAccount,
  getColabSettings,
  stopColabRuntime,
  type KaggleAccount,
  type KaggleSettings,
  getKaggleSettings,
  addKaggleAccount,
  importKaggleAccount,
  setKaggleActiveAccount,
  removeKaggleAccount,
} from "../../api";
import { m } from "../../paraglide/messages.js";
import {
  Badge,
  Button,
  IconButton,
  Input,
  LoadingRow,
  Spinner,
} from "../../components/ui";
import { COMPUTE_DIAGNOSTIC_CLASS_NAME, FORM_CLASS_NAME, KV_CLASS_NAME } from "../../components/SettingsPage";

// --- compute (colab) --------------------------------------------------------------

function colabTierLabel(tier: ColabAccount["tier"]): string | null {
  switch (tier) {
    case "free":
      return m.settings_colab_tier_free();
    case "pro":
      return m.settings_colab_tier_pro();
    case "pro_plus":
      return m.settings_colab_tier_pro_plus();
    default:
      return null;
  }
}

export const fmtUnits = (value: number) => value.toLocaleString(undefined, { maximumFractionDigits: 2, minimumFractionDigits: 2 });

export function ColabSection() {
  const options = getColabSettingsQuery();
  const query = useQuery(options);
  const s: ColabSettings | null = query.data ?? null;
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loading = query.isPending || refreshing;

  async function refresh() {
    if (loading) return;
    setRefreshing(true);
    setError(null);
    try {
      const fresh = await getColabSettings(undefined, true);
      queryClient.setQueryData(options.queryKey, fresh);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }

  const [stopping, setStopping] = useState<string | null>(null);
  async function stopRuntime(endpoint: string) {
    if (!window.confirm(m.settings_colab_stop_confirm())) return;
    setStopping(endpoint);
    setError(null);
    try {
      await stopColabRuntime(endpoint);
      queryClient.setQueryData(options.queryKey, await getColabSettings(undefined, true));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStopping(null);
    }
  }

  const loadError = error ?? query.error?.message ?? null;
  const account = s?.account ?? null;
  const tier = colabTierLabel(account?.tier ?? null);
  const rates = new Map((account?.rates ?? []).map((rate) => [rate.id, rate]));
  const eligible = account?.eligible ? new Set(account.eligible) : null;
  return (
    <>
      {!s && loading ? (
        <LoadingRow>
          <Spinner /> {m.settings_colab_checking()}
        </LoadingRow>
      ) : s && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant={s.ready ? "success" : "warning"}>
              {s.ready
                ? m.settings_page_ready_to_use()
                : !s.cliPath
                  ? m.settings_colab_not_installed()
                  : m.settings_colab_not_signed_in()}
            </Badge>
            {tier && <Badge>{tier}</Badge>}
            {account?.email && <span className="truncate text-sm text-subtext" title={m.settings_colab_google_account()}>{account.email}</span>}
            <div className="flex-1" />
            <Button size="small" onClick={() => void refresh()} disabled={loading}>
              <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
            </Button>
          </div>
          {s.error && <p className="mt-3 mb-0 text-sm text-subtext">{s.error}</p>}
          {account && (
            <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="rounded-lg border border-border bg-surface px-4 py-3">
                <div className="text-sm text-subtext">{m.settings_colab_compute_units()}</div>
                <div className="mt-1 text-2xl font-semibold tabular-nums text-text">
                  {account.balance != null ? fmtUnits(account.balance) : "—"}
                </div>
              </div>
              <div className="rounded-lg border border-border bg-surface px-4 py-3">
                <div className="text-sm text-subtext">{m.settings_colab_using_now()}</div>
                <div className="mt-1 text-2xl font-semibold tabular-nums text-text">
                  {account.rateHourly != null && account.rateHourly > 0
                    ? m.settings_colab_rate_value({ rate: fmtUnits(account.rateHourly) })
                    : account.rateHourly != null ? m.settings_colab_idle() : "—"}
                </div>
                {account.activeRuntimes != null && account.activeRuntimes > 0 && (
                  <div className="mt-0.5 text-sm text-subtext">{m.settings_colab_active_runtimes({ count: fmtNumber(account.activeRuntimes) })}</div>
                )}
              </div>
            </div>
          )}
          {account?.error && <p className="mt-3 mb-0 text-sm text-subtext">{account.error}</p>}
          {s.runtimes && (
            <div className="mt-4">
              <p className="mt-0 mb-2 text-sm font-medium text-subtext">{m.settings_colab_runtimes()}</p>
              {s.runtimes.length === 0 ? (
                <p className="m-0 text-sm text-subtext">{m.settings_colab_no_runtimes()}</p>
              ) : (
                <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
                  {s.runtimes.map((runtime) => (
                    <li key={runtime.endpoint} className="flex items-center gap-3 rounded-md border border-border px-3 py-2">
                      <code>{runtime.accelerator ?? "?"}{runtime.highMem ? ":highmem" : ""}</code>
                      <span className="min-w-0 flex-1 truncate text-subtext">
                        {runtime.orphaned
                          ? m.settings_colab_runtime_orphaned()
                          : runtime.runId
                            ? m.settings_colab_runtime_run({ run: runtime.runId.slice(0, 8), status: runtime.runStatus ?? "?" })
                            : runtime.name ?? m.settings_colab_runtime_external()}
                      </span>
                      <Button
                        size="small"
                        disabled={stopping === runtime.endpoint}
                        onClick={() => void stopRuntime(runtime.endpoint)}
                      >
                        {stopping === runtime.endpoint ? <Spinner /> : <Square size={12} />} {m.settings_colab_stop()}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
              <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_colab_auto_release()}</p>
            </div>
          )}
          <div className="mt-4 overflow-x-auto">
            <table className="colab-accelerators w-full border-collapse text-sm">
              <thead>
                <tr className="text-start text-subtext [&_th]:border-b [&_th]:border-border [&_th]:pb-1.5 [&_th]:font-medium">
                  <th className="text-start">{m.settings_colab_col_accelerator()}</th>
                  <th className="text-end">{m.settings_colab_col_memory()}</th>
                  <th className="text-end">{m.settings_colab_col_rate()}</th>
                  {account?.balance != null && <th className="text-end">{m.settings_colab_col_hours()}</th>}
                  {eligible && <th className="text-end">{m.settings_colab_col_plan()}</th>}
                </tr>
              </thead>
              <tbody>
                {s.accelerators.map((accelerator) => {
                  const rate = rates.get(accelerator.id);
                  const usable = eligible ? eligible.has(accelerator.id) : null;
                  const hours = rate && account?.balance != null && rate.cuPerHour > 0 ? account.balance / rate.cuPerHour : null;
                  return (
                    <tr
                      key={accelerator.id}
                      className={cn("[&_td]:border-b [&_td]:border-border [&_td]:py-1.5", usable === false && "text-subtext")}
                    >
                      <td>
                        <code className="me-2">{accelerator.id}</code>
                        {accelerator.label}
                      </td>
                      <td className="text-end tabular-nums">
                        {accelerator.memoryGb > 0 ? m.settings_colab_memory_gb({ gb: fmtNumber(accelerator.memoryGb) }) : "—"}
                      </td>
                      <td className="text-end tabular-nums">
                        {rate ? (
                          <span title={rate.measured ? m.settings_colab_measured() : undefined}>
                            {rate.measured ? "" : "≈ "}
                            {fmtUnits(rate.cuPerHour)}
                            {rate.measured && <span className="ms-1 text-accent-green">●</span>}
                          </span>
                        ) : "—"}
                      </td>
                      {account?.balance != null && (
                        <td className="text-end tabular-nums">
                          {hours != null ? m.settings_colab_hours_value({ hours: hours.toLocaleString(undefined, { maximumFractionDigits: 1 }) }) : "—"}
                        </td>
                      )}
                      {eligible && (
                        <td className="text-end">
                          <Badge size="small" variant={usable ? "success" : "default"}>
                            {usable ? m.settings_colab_available() : m.settings_colab_unavailable()}
                          </Badge>
                        </td>
                      )}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <p className="mt-3 mb-0 text-sm text-subtext">{m.settings_colab_rates_note()}</p>
          <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_colab_flavor_hint({ flavor: s.defaultFlavor })}</p>
          <div className={cn(KV_CLASS_NAME, "mt-4 [&_.v]:text-sm")}>
            <span className="k">{m.settings_colab_cli()}</span>
            <span className="v">{s.cliPath ?? m.settings_not_found_on_path()}</span>
            <span className="k">{m.settings_page_version()}</span>
            <span className="v">{s.cliVersion ?? "—"}</span>
          </div>
        </>
      )}
      {loadError && <div className="error mt-3">{loadError}</div>}
    </>
  );
}

// --- compute (kaggle) --------------------------------------------------------------

const fmtHours = (hours: number) => hours.toLocaleString(undefined, { maximumFractionDigits: 1 });

function kaggleQuotaText(line: { remainingHours: number; totalHours: number } | null | undefined): string {
  return line ? m.settings_kaggle_quota_left({ left: fmtHours(line.remainingHours), total: fmtHours(line.totalHours) }) : "—";
}

function kaggleMethodLabel(method: KaggleAccount["method"]): string {
  switch (method) {
    case "token":
      return m.settings_kaggle_method_token();
    case "key":
      return m.settings_kaggle_method_key();
    default:
      return m.settings_kaggle_method_oauth();
  }
}

/** Kaggle: CLI install guidance, several saved accounts, and each account's
 * weekly GPU/TPU quota. Credentials are sent once and never shown again. */
export function KaggleSection() {
  const options = getKaggleSettingsQuery();
  const query = useQuery(options);
  const s: KaggleSettings | null = query.data ?? null;
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const loading = query.isPending || refreshing;

  async function run(label: string, action: () => Promise<KaggleSettings>) {
    setBusy(label);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await action());
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setBusy(null);
    }
  }

  async function refresh() {
    if (loading) return;
    setRefreshing(true);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await getKaggleSettings(undefined, true));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }

  async function add() {
    const value = token.trim();
    if (!value) return;
    // A pasted kaggle.json carries its own username.
    const account = value.startsWith("{")
      ? { kaggleJson: value }
      : { token: value, username: username.trim() || undefined };
    if (await run("add", () => addKaggleAccount(account))) {
      setToken("");
      setUsername("");
    }
  }

  const loadError = error ?? query.error?.message ?? null;
  return (
    <>
      {!s && loading ? (
        <LoadingRow>
          <Spinner /> {m.common_checking()}
        </LoadingRow>
      ) : s && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant={s.ready ? "success" : "warning"}>
              {s.ready
                ? m.settings_page_ready_to_use()
                : !s.cliPath
                  ? m.settings_kaggle_not_installed()
                  : m.settings_kaggle_no_account()}
            </Badge>
            {s.accountCount > 0 && <span className="text-sm text-subtext">{m.settings_kaggle_account_count({ count: fmtNumber(s.accountCount) })}</span>}
            <div className="flex-1" />
            <Button size="small" onClick={() => void refresh()} disabled={loading}>
              <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {s.cliPath ? m.settings_page_refresh() : m.settings_check_again()}
            </Button>
          </div>
          {!s.supportedOs && <p className={COMPUTE_DIAGNOSTIC_CLASS_NAME}>{s.error}</p>}
          {s.supportedOs && !s.cliPath && (
            <div className="mt-4 rounded-lg border border-border bg-surface px-4 py-3">
              <div className="text-sm font-medium text-text">{m.settings_kaggle_install_title()}</div>
              <p className="mt-1 mb-2 text-sm text-subtext">{m.settings_kaggle_install_body()}</p>
              <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
                {s.installCommands.map((command, index) => (
                  <li key={command} className="flex items-center gap-2">
                    <code className="rounded-md border border-border bg-background px-2 py-1">{command}</code>
                    {index === 0 && <span className="text-subtext">{m.settings_kaggle_install_recommended()}</span>}
                  </li>
                ))}
              </ul>
              <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_kaggle_install_then()}</p>
            </div>
          )}
          {s.cliOutdated && <p className="mt-3 mb-0 text-sm text-subtext">{m.settings_kaggle_outdated()} <code>uv tool upgrade kaggle</code></p>}
          {s.cliPath && (
            <>
              <p className="mt-4 mb-2 text-sm font-medium text-subtext">{m.settings_kaggle_accounts()}</p>
              {s.accounts.length === 0 ? (
                <p className="m-0 text-sm text-subtext">{m.settings_kaggle_no_accounts()}</p>
              ) : (
                <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
                  {s.accounts.map((account) => (
                    <li key={account.username} className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-border px-3 py-2">
                      <span className="font-medium text-text">{account.username}</span>
                      <Badge size="small">{kaggleMethodLabel(account.method)}</Badge>
                      {account.active && <Badge size="small" variant="success">{m.settings_kaggle_default()}</Badge>}
                      <span className="min-w-0 flex-1 truncate text-subtext">
                        {account.error
                          ? account.error
                          : account.quota
                            ? `${m.settings_kaggle_gpu()} ${kaggleQuotaText(account.quota.gpu)} · ${m.settings_kaggle_tpu()} ${kaggleQuotaText(account.quota.tpu)}`
                            : m.settings_kaggle_quota_unknown()}
                      </span>
                      {!account.active && (
                        <Button size="small" variant="ghost" disabled={busy != null} onClick={() => void run(`active:${account.username}`, () => setKaggleActiveAccount(account.username))}>
                          {m.settings_kaggle_make_default()}
                        </Button>
                      )}
                      <IconButton
                        title={m.settings_kaggle_remove()}
                        aria-label={m.settings_kaggle_remove()}
                        disabled={busy != null}
                        onClick={() => {
                          if (!window.confirm(m.settings_kaggle_remove_confirm({ name: account.username }))) return;
                          void run(`remove:${account.username}`, () => removeKaggleAccount(account.username));
                        }}
                      >
                        {busy === `remove:${account.username}` ? <Spinner /> : <Trash2 size={13} />}
                      </IconButton>
                    </li>
                  ))}
                </ul>
              )}
              {s.accounts.some((account) => account.quota?.refreshAt) && (
                <p className="mt-2 mb-0 text-sm text-subtext">
                  {m.settings_kaggle_quota_resets({ date: new Date(s.accounts.find((account) => account.quota?.refreshAt)?.quota?.refreshAt ?? "").toLocaleString() })}
                </p>
              )}
              {s.importable && (
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  <Button size="small" disabled={busy != null} onClick={() => void run("import", importKaggleAccount)}>
                    {busy === "import" ? <Spinner /> : <Plus size={12} />} {m.settings_kaggle_import()}
                  </Button>
                  <span className="text-sm text-subtext">{m.settings_kaggle_import_hint()}</span>
                </div>
              )}
              <form
                className={cn(FORM_CLASS_NAME, "mt-4")}
                onSubmit={(event) => {
                  event.preventDefault();
                  void add();
                }}
              >
                <label>
                  {m.settings_kaggle_token()}
                  <Input
                    type="password"
                    value={token}
                    onChange={(event) => setToken(event.target.value)}
                    placeholder="KGAT_…"
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
                <label>
                  {m.settings_kaggle_username()}
                  <Input
                    type="text"
                    value={username}
                    onChange={(event) => setUsername(event.target.value)}
                    placeholder={m.settings_kaggle_username_placeholder()}
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
                <p className="m-0 text-sm text-subtext">
                  {m.settings_kaggle_token_hint()}{" "}
                  <a className="underline" href="https://www.kaggle.com/settings/api" target="_blank" rel="noreferrer">
                    kaggle.com/settings/api
                  </a>
                </p>
                <div className="actions">
                  <Button variant="primary" type="submit" disabled={busy != null || !token.trim()}>
                    {busy === "add" ? <Spinner /> : <Plus size={13} />} {m.settings_kaggle_add()}
                  </Button>
                </div>
              </form>
            </>
          )}
          <div className="mt-4 overflow-x-auto">
            <table className="kaggle-accelerators w-full border-collapse text-sm">
              <thead>
                <tr className="text-start text-subtext [&_th]:border-b [&_th]:border-border [&_th]:pb-1.5 [&_th]:font-medium">
                  <th className="text-start">{m.settings_colab_col_accelerator()}</th>
                  <th className="text-end">{m.settings_colab_col_memory()}</th>
                </tr>
              </thead>
              <tbody>
                {s.accelerators.map((accelerator) => (
                  <tr key={accelerator.id} className="[&_td]:border-b [&_td]:border-border [&_td]:py-1.5">
                    <td>
                      <code className="me-2">{accelerator.id}</code>
                      {accelerator.label}
                    </td>
                    <td className="text-end tabular-nums">
                      {accelerator.memoryGb > 0 ? m.settings_colab_memory_gb({ gb: fmtNumber(accelerator.memoryGb) }) : "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-3 mb-0 text-sm text-subtext">{m.settings_kaggle_flavor_hint({ flavor: s.defaultFlavor })}</p>
          <div className={cn(KV_CLASS_NAME, "mt-4 [&_.v]:text-sm")}>
            <span className="k">{m.settings_kaggle_cli()}</span>
            <span className="v">{s.cliPath ?? m.settings_not_found_on_path()}</span>
            <span className="k">{m.settings_page_version()}</span>
            <span className="v">{s.cliVersion ?? "—"}</span>
          </div>
        </>
      )}
      {loadError && <div className="error mt-3">{loadError}</div>}
    </>
  );
}

