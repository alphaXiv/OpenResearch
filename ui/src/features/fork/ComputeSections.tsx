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
  ExternalLink,
  Pause,
  Play,
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
  type ColabSavedAccount,
  getColabSettings,
  stopColabRuntime,
  startColabLogin,
  finishColabLogin,
  cancelColabLogin,
  importColabAccount,
  setColabActiveAccount,
  pauseColabAccount,
  removeColabAccount,
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
  ButtonLink,
  IconButton,
  Input,
  LoadingRow,
  Spinner,
  StatusIndicator,
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

function colabAcceleratorLabel(id: string, accelerators: ColabSettings["accelerators"]): string {
  return accelerators.find((accelerator) => accelerator.id === id)?.cliValue || id.toUpperCase();
}

/** One saved Google account: plan, compute units, what it can run, its runtimes. */
function ColabAccountCard({
  saved,
  accelerators,
  busy,
  stopping,
  onAction,
  onStop,
}: {
  saved: ColabSavedAccount;
  accelerators: ColabSettings["accelerators"];
  busy: string | null;
  stopping: string | null;
  onAction: (label: string, action: () => Promise<ColabSettings>) => void;
  onStop: (account: string, endpoint: string) => void;
}) {
  const account = saved.account;
  const tier = colabTierLabel(account?.tier ?? null);
  const gpus = (account?.eligible ?? []).filter((id) => id !== "cpu").map((id) => colabAcceleratorLabel(id, accelerators));
  return (
    <li className={cn("rounded-md border border-border px-3 py-2", saved.paused && "opacity-70")}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="min-w-0 truncate font-medium text-text" title={m.settings_colab_google_account()}>{saved.id}</span>
        {tier && <Badge size="small">{tier}</Badge>}
        {saved.active && <Badge size="small" variant="success">{m.settings_colab_default()}</Badge>}
        {saved.paused && <Badge size="small" variant="warning">{m.settings_colab_paused()}</Badge>}
        <div className="flex-1" />
        {!saved.active && (
          <Button size="small" variant="ghost" disabled={busy != null} onClick={() => onAction(`active:${saved.id}`, () => setColabActiveAccount(saved.id))}>
            {m.settings_colab_make_default()}
          </Button>
        )}
        <Button
          size="small"
          variant="ghost"
          title={m.settings_colab_pause_hint()}
          disabled={busy != null}
          onClick={() => onAction(`pause:${saved.id}`, () => pauseColabAccount(saved.id, !saved.paused))}
        >
          {busy === `pause:${saved.id}` ? <Spinner /> : saved.paused ? <Play size={12} /> : <Pause size={12} />}{" "}
          {saved.paused ? m.settings_colab_resume() : m.settings_colab_pause()}
        </Button>
        <IconButton
          title={m.settings_colab_remove()}
          aria-label={m.settings_colab_remove()}
          disabled={busy != null}
          onClick={() => {
            if (!window.confirm(m.settings_colab_remove_confirm({ name: saved.id }))) return;
            onAction(`remove:${saved.id}`, () => removeColabAccount(saved.id));
          }}
        >
          {busy === `remove:${saved.id}` ? <Spinner /> : <Trash2 size={13} />}
        </IconButton>
      </div>
      {account && (
        <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-sm text-subtext">
          <span>
            {m.settings_colab_compute_units()}:{" "}
            <span className="tabular-nums text-text">{account.balance != null ? fmtUnits(account.balance) : "—"}</span>
          </span>
          <span>
            {m.settings_colab_using_now()}:{" "}
            <span className="tabular-nums text-text">
              {account.rateHourly != null && account.rateHourly > 0
                ? m.settings_colab_rate_value({ rate: fmtUnits(account.rateHourly) })
                : account.rateHourly != null ? m.settings_colab_idle() : "—"}
            </span>
          </span>
          {account.eligible && (
            <span>{gpus.length > 0 ? m.settings_colab_can_run({ list: gpus.join(", ") }) : m.settings_colab_cpu_only()}</span>
          )}
        </div>
      )}
      {account?.error && <p className="mt-1 mb-0 text-sm text-accent-red">{account.error}</p>}
      {saved.runtimes && saved.runtimes.length > 0 && (
        <ul className="m-0 mt-2 flex list-none flex-col gap-1.5 p-0 text-sm">
          {saved.runtimes.map((runtime) => (
            <li key={runtime.endpoint} className="flex items-center gap-3 rounded-md border border-border bg-surface px-3 py-1.5">
              <StatusIndicator tone={runtime.orphaned ? "warning" : "success"} live={!runtime.orphaned} />
              <code>{runtime.accelerator ?? "?"}{runtime.highMem ? ":highmem" : ""}</code>
              <span className="min-w-0 flex-1 truncate text-subtext">
                {runtime.orphaned
                  ? m.settings_colab_runtime_orphaned()
                  : runtime.runId
                    ? m.settings_colab_runtime_run({ run: runtime.runId.slice(0, 8), status: runtime.runStatus ?? "?" })
                    : runtime.name ?? m.settings_colab_runtime_external()}
              </span>
              <Button size="small" disabled={stopping === runtime.endpoint} onClick={() => onStop(saved.id, runtime.endpoint)}>
                {stopping === runtime.endpoint ? <Spinner /> : <Square size={12} />} {m.settings_colab_stop()}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}

/** Adding a Google account: the Colab CLI's own sign-in, with Google's code
 * pasted back here. Each sign-in adds one account. */
function ColabAddAccount({ busy, onSaved }: { busy: boolean; onSaved: (settings: ColabSettings) => void }) {
  const [login, setLogin] = useState<{ id: string; url: string } | null>(null);
  const [code, setCode] = useState("");
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function start() {
    setWorking(true);
    setError(null);
    try {
      setLogin(await startColabLogin());
      setCode("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setWorking(false);
    }
  }

  async function finish() {
    if (!login || !code.trim()) return;
    setWorking(true);
    setError(null);
    try {
      onSaved(await finishColabLogin(login.id, code.trim()));
      setLogin(null);
      setCode("");
    } catch (err) {
      // The CLI exits after a wrong code, so the next try starts over.
      setLogin(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setWorking(false);
    }
  }

  function cancel() {
    if (login) void cancelColabLogin(login.id).catch(() => undefined);
    setLogin(null);
    setCode("");
  }

  return (
    <div className="mt-3">
      {!login ? (
        <Button size="small" variant="primary" disabled={busy || working} onClick={() => void start()}>
          {working ? <Spinner /> : <Plus size={12} />} {m.settings_colab_add()}
        </Button>
      ) : (
        <form
          className={FORM_CLASS_NAME}
          onSubmit={(event) => {
            event.preventDefault();
            void finish();
          }}
        >
          <p className="m-0 text-sm text-subtext">{m.settings_colab_login_hint()}</p>
          <div>
            <ButtonLink href={login.url} target="_blank" rel="noreferrer">
              {m.settings_colab_login_open()} <ExternalLink size={12} />
            </ButtonLink>
          </div>
          <label>
            {m.settings_colab_login_code()}
            <Input
              type="text"
              value={code}
              onChange={(event) => setCode(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <div className="actions">
            <Button type="button" onClick={cancel} disabled={working}>
              {m.settings_page_cancel()}
            </Button>
            <Button variant="primary" type="submit" disabled={working || !code.trim()}>
              {working ? <Spinner /> : <Plus size={13} />} {m.settings_colab_login_finish()}
            </Button>
          </div>
        </form>
      )}
      {error && <div className="error mt-2">{error}</div>}
    </div>
  );
}

export function ColabSection() {
  const options = getColabSettingsQuery();
  const query = useQuery(options);
  const s: ColabSettings | null = query.data ?? null;
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
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

  async function run(label: string, action: () => Promise<ColabSettings>) {
    setBusy(label);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await action());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
    }
  }

  const [stopping, setStopping] = useState<string | null>(null);
  async function stopRuntime(account: string, endpoint: string) {
    if (!window.confirm(m.settings_colab_stop_confirm())) return;
    setStopping(endpoint);
    setError(null);
    try {
      await stopColabRuntime(endpoint, account);
      queryClient.setQueryData(options.queryKey, await getColabSettings(undefined, true));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStopping(null);
    }
  }

  const loadError = error ?? query.error?.message ?? null;
  const accounts = s?.accounts ?? [];
  // The table sums what unpaused accounts can run, as @auto would pick.
  const usable = accounts.filter((saved) => !saved.paused).map((saved) => saved.account).filter((account): account is ColabAccount => account != null);
  const rates = new Map((usable[0]?.rates ?? s?.account?.rates ?? []).map((rate) => [rate.id, rate]));
  const balances = usable.map((account) => account.balance).filter((balance): balance is number => balance != null);
  const balance = balances.length > 0 ? balances.reduce((sum, value) => sum + value, 0) : null;
  const eligibleLists = usable.map((account) => account.eligible).filter((list): list is string[] => list != null);
  const eligible = eligibleLists.length > 0 ? new Set(eligibleLists.flat()) : null;
  const anyRuntime = accounts.some((saved) => (saved.runtimes?.length ?? 0) > 0);
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
                  : m.settings_colab_no_account()}
            </Badge>
            {accounts.length > 1 && <span className="text-sm text-subtext">{m.settings_colab_account_count({ count: fmtNumber(accounts.length) })}</span>}
            <div className="flex-1" />
            <Button size="small" onClick={() => void refresh()} disabled={loading}>
              <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
            </Button>
          </div>
          {s.error && <p className="mt-3 mb-0 text-sm text-subtext">{s.error}</p>}
          {s.cliPath && s.supportedOs && (
            <>
              <p className="mt-4 mb-2 text-sm font-medium text-subtext">{m.settings_colab_accounts()}</p>
              {accounts.length === 0 ? (
                <p className="m-0 text-sm text-subtext">{m.settings_colab_no_accounts()}</p>
              ) : (
                <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
                  {accounts.map((saved) => (
                    <ColabAccountCard
                      key={saved.id}
                      saved={saved}
                      accelerators={s.accelerators}
                      busy={busy}
                      stopping={stopping}
                      onAction={(label, action) => void run(label, action)}
                      onStop={(account, endpoint) => void stopRuntime(account, endpoint)}
                    />
                  ))}
                </ul>
              )}
              {anyRuntime ? (
                <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_colab_auto_release()}</p>
              ) : accounts.length > 0 && (
                <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_colab_no_runtimes()}</p>
              )}
              {s.importable && accounts.length === 0 && (
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  <Button size="small" disabled={busy != null} onClick={() => void run("import", importColabAccount)}>
                    {busy === "import" ? <Spinner /> : <Plus size={12} />} {m.settings_colab_import()}
                  </Button>
                  <span className="text-sm text-subtext">{m.settings_colab_import_hint()}</span>
                </div>
              )}
              <ColabAddAccount busy={busy != null} onSaved={(settings) => queryClient.setQueryData(options.queryKey, settings)} />
            </>
          )}
          <div className="mt-4 overflow-x-auto">
            <table className="colab-accelerators w-full border-collapse text-sm">
              <thead>
                <tr className="text-start text-subtext [&_th]:border-b [&_th]:border-border [&_th]:pb-1.5 [&_th]:font-medium">
                  <th className="text-start">{m.settings_colab_col_accelerator()}</th>
                  <th className="text-end">{m.settings_colab_col_memory()}</th>
                  <th className="text-end">{m.settings_colab_col_rate()}</th>
                  {balance != null && <th className="text-end">{m.settings_colab_col_hours()}</th>}
                  {eligible && <th className="text-end">{m.settings_colab_col_plan()}</th>}
                </tr>
              </thead>
              <tbody>
                {s.accelerators.map((accelerator) => {
                  const rate = rates.get(accelerator.id);
                  const available = eligible ? eligible.has(accelerator.id) : null;
                  const hours = rate && balance != null && rate.cuPerHour > 0 ? balance / rate.cuPerHour : null;
                  return (
                    <tr
                      key={accelerator.id}
                      className={cn(
                        "transition-colors hover:bg-surface [&_td]:border-b [&_td]:border-border [&_td]:py-1.5",
                        available === false && "text-subtext",
                      )}
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
                      {balance != null && (
                        <td className="text-end tabular-nums">
                          {hours != null ? m.settings_colab_hours_value({ hours: hours.toLocaleString(undefined, { maximumFractionDigits: 1 }) }) : "—"}
                        </td>
                      )}
                      {eligible && (
                        <td className="text-end">
                          <Badge size="small" variant={available ? "success" : "default"}>
                            {available ? m.settings_colab_available() : m.settings_colab_unavailable()}
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

function KaggleQuotaMeter({
  label,
  quota,
}: {
  label: string;
  quota: { remainingHours: number; totalHours: number } | null | undefined;
}) {
  const pct =
    quota && quota.totalHours > 0
      ? Math.max(0, Math.min(100, Math.round((quota.remainingHours / quota.totalHours) * 100)))
      : null;
  const fillClass =
    pct == null
      ? "bg-muted"
      : pct <= 10
        ? "bg-accent-red"
        : pct <= 30
          ? "bg-accent-amber"
          : "bg-accent-green";
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-subtext">
      <span className="font-medium text-text">{label}</span>
      {pct != null && (
        <span aria-hidden="true" className="h-1.5 w-14 shrink-0 overflow-hidden rounded-full bg-panel">
          <span
            className={cn("block h-full rounded-full transition-[width] duration-150", fillClass)}
            style={{ width: `${pct}%` }}
          />
        </span>
      )}
      <span className="tabular-nums">{kaggleQuotaText(quota)}</span>
    </span>
  );
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
                    <li key={account.username} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md border border-border px-3 py-2">
                      <span className="font-medium text-text">{account.username}</span>
                      <Badge size="small">{kaggleMethodLabel(account.method)}</Badge>
                      {account.active && <Badge size="small" variant="success">{m.settings_kaggle_default()}</Badge>}
                      <div className="min-w-0 flex-1 text-subtext">
                        {account.error ? (
                          <span className="text-accent-red">{account.error}</span>
                        ) : account.quota ? (
                          <span className="flex flex-wrap items-center gap-x-4 gap-y-1">
                            <KaggleQuotaMeter label={m.settings_kaggle_gpu()} quota={account.quota.gpu} />
                            <KaggleQuotaMeter label={m.settings_kaggle_tpu()} quota={account.quota.tpu} />
                          </span>
                        ) : (
                          m.settings_kaggle_quota_unknown()
                        )}
                      </div>
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
                  <tr key={accelerator.id} className="transition-colors hover:bg-surface [&_td]:border-b [&_td]:border-border [&_td]:py-1.5">
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

