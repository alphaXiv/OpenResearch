// Harness accounts, routing, Decision model, provider prices and GCP settings
// cards, owned by the tuandang304 fork. Kept out of SettingsPage.tsx so upstream
// edits there merge cleanly; see docs/fork-maintenance.md.
import { cn } from "../../components/ui/cn";
import { TARGET_LABELS } from "../../computeTargets";
import {
  setScopedQueryData,
  queryClient,
} from "../../queries/client";
import { useMutation, useQuery } from "@tanstack/react-query";

import {
  getHarnessAccountsQuery,
  getGcpSettingsQuery,
  getCrossHarnessSettingsQuery,
  getComputeRoutingSettingsQuery,
  getAutoCompactSettingsQuery,
  getComputePricesQuery,
  getDecisionModelSettingsQuery,
} from "../../queries/settings";

import { getUiStateQuery } from "../../queries/projects";
import {
  ArrowDown,
  ArrowUp,
  Plus,
  RefreshCw,
  Square,
  SquareTerminal,
  Trash2,
} from "lucide-react";
import { useEffect, useState } from "react";
import {
  fmtNumber,
  type ComputeTargetId,
  type ComputeTargetSummary,
  type Harness,
  type HarnessFailover,
  type HarnessAccount,
  type HarnessAccounts,
  type HarnessPoolLimit,
  type HarnessPoolStatus,
  getHarnessAccounts,
  addHarnessAccount,
  updateHarnessAccount,
  removeHarnessAccount,
  harnessAccountLoginPath,
  type HarnessId,
  type DecisionModelSettings,
  saveDecisionModelSettings,
  getGcpSettings,
  saveGcpSettings,
  stopGcpInstance,
  type GcpSettings,
  type GcpSettingsUpdate,
  getComputePrices,
  updateUiState,
  setCrossHarnessSettings,
  setComputeRoutingSettings,
  setAutoCompactSettings,
  fmtTokens,
} from "../../api";
import { m } from "../../paraglide/messages.js";
import { fmtUnits } from "../../features/fork/ComputeSections";
import { ltr } from "../../i18n";
import { BackendLogo } from "../../components/BackendLogos";
import { OptionPicker } from "../../components/ModelPicker";
import { HarnessLogo } from "../../components/HarnessLogo";
import {
  Badge,
  Button,
  IconButton,
  Input,
  LoadingRow,
  showAlert,
  Spinner,
  Switch,
} from "../../components/ui";
import { COMPUTE_DIAGNOSTIC_CLASS_NAME, CommandRunTerminal, FORM_CLASS_NAME, PROJECT_DEFAULT_ROW_CLASS_NAME, SETTINGS_CARD_CLASS_NAME, TARGET_KIND, harnessStatus, useCommandRun } from "../../components/SettingsPage";

export const ACCOUNT_HARNESSES: HarnessId[] = ["claude-code", "codex", "antigravity"];

function fmtClock(ms: number): string {
  const date = new Date(ms);
  return ms - Date.now() < 20 * 3600_000 ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : date.toLocaleString();
}

/** One line per pool: share left, the provider's windows, a set limit, a block. */
function poolSummary(pool: HarnessPoolStatus): string {
  const parts: string[] = [
    pool.blockedUntil
      ? m.settings_accounts_blocked_until({ time: fmtClock(pool.blockedUntil) })
      : pool.remaining != null
        ? m.settings_accounts_left({ percent: fmtNumber(Math.round(pool.remaining * 100)) })
        : m.settings_accounts_quota_unknown(),
  ];
  for (const window of pool.windows) {
    const used = m.settings_accounts_window({ window: window.window, percent: fmtNumber(Math.round(window.usedPercent)) });
    parts.push(window.resetsAt ? `${used}, ${m.settings_accounts_resets({ time: fmtClock(window.resetsAt) })}` : used);
  }
  if (pool.limit && pool.used != null) {
    parts.push(m.settings_accounts_used_of({
      used: fmtNumber(Math.round(pool.used)),
      limit: fmtNumber(pool.limit.limit),
      unit: pool.limit.unit === "turns" ? m.settings_accounts_unit_turns() : m.settings_accounts_unit_tokens(),
      hours: fmtNumber(pool.limit.windowHours),
    }));
  }
  parts.push(m.settings_accounts_tracked({ turns: fmtNumber(pool.trackedTurns) }));
  return parts.join(" · ");
}

const QUOTA_DOT: Record<HarnessPoolStatus["state"], string> = {
  ok: "bg-accent-green",
  low: "bg-accent-amber",
  blocked: "bg-accent-red",
  unknown: "bg-muted",
};

/** Several accounts for one harness: each one's quota per pool, sign-in,
 * pausing, limits set by hand, and adding or removing accounts. */
export function HarnessAccountsCard({ harness, remote }: { harness: Harness; remote: boolean }) {
  const options = getHarnessAccountsQuery();
  const query = useQuery(options);
  const group: HarnessAccounts | undefined = query.data?.find((entry) => entry.id === harness.id);
  const [busy, setBusy] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [newId, setNewId] = useState("");
  const [newLabel, setNewLabel] = useState("");
  const loginRun = useCommandRun();

  async function run(label: string, action: () => Promise<HarnessAccounts[]>) {
    setBusy(label);
    try {
      queryClient.setQueryData(options.queryKey, await action());
      return true;
    } catch (err) {
      showAlert(err instanceof Error ? err.message : String(err), "error");
      return false;
    } finally {
      setBusy(null);
    }
  }

  const signIn = (account: HarnessAccount) =>
    loginRun.start(account.loginCommand ?? account.id, harnessAccountLoginPath(harness.id, account.id), account.id);

  async function add() {
    const id = newId.trim().toLowerCase();
    if (!id) return;
    if (await run("add", () => addHarnessAccount(harness.id, id, newLabel.trim() || undefined))) {
      setNewId("");
      setNewLabel("");
      const added = queryClient.getQueryData(options.queryKey)?.find((entry) => entry.id === harness.id)?.accounts.find((account) => account.id === id);
      if (added && !remote) signIn(added);
    }
  }

  if (!group) {
    return query.isPending ? (
      <LoadingRow>
        <Spinner /> {m.common_checking()}
      </LoadingRow>
    ) : null;
  }
  return (
    <div className={SETTINGS_CARD_CLASS_NAME}>
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-base font-medium">{m.settings_accounts_title()}</div>
          <p className="mt-1 mb-0 text-sm text-subtext">{m.settings_accounts_description()}</p>
        </div>
        <Button size="small" disabled={busy != null} onClick={() => void run("refresh", () => getHarnessAccounts(undefined, true))}>
          <RefreshCw size={12} className={busy === "refresh" ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_accounts_refresh()}
        </Button>
      </div>
      <p className="mt-2 mb-0 text-sm text-subtext">{group.quotaSource}</p>
      <ul className="mt-3 mb-0 flex list-none flex-col gap-1.5 p-0 text-sm">
        {group.accounts.map((account) => {
          const name = account.isDefault ? m.settings_accounts_default() : account.label ? `${account.id} · ${account.label}` : account.id;
          return (
            <li key={account.id} className="rounded-md border border-border px-3 py-2">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="font-medium text-text">{name}</span>
                {account.email && <span className="min-w-0 truncate text-subtext">{account.email}</span>}
                {!account.signedIn && <Badge size="small" variant="warning">{m.settings_accounts_not_signed_in()}</Badge>}
                {account.paused && <Badge size="small">{m.settings_accounts_paused()}</Badge>}
                <span className="flex-1" />
                {!account.isDefault && !remote && (
                  <Button size="small" variant="ghost" disabled={busy != null} onClick={() => signIn(account)}>
                    <SquareTerminal size={13} /> {m.settings_accounts_sign_in()}
                  </Button>
                )}
                {account.pools.some((pool) => pool.blockedUntil) && (
                  <Button size="small" variant="ghost" disabled={busy != null} onClick={() => void run(`clear:${account.id}`, () => updateHarnessAccount(harness.id, account.id, { clearBlock: true }))}>
                    {m.settings_accounts_clear_block()}
                  </Button>
                )}
                <Button size="small" variant="ghost" disabled={busy != null} onClick={() => setEditing(editing === account.id ? null : account.id)}>
                  {m.settings_accounts_limits()}
                </Button>
                <Switch
                  type="button"
                  checked={!account.paused}
                  aria-label={m.settings_accounts_use({ name: ltr(name) })}
                  disabled={busy != null}
                  onClick={() => void run(`pause:${account.id}`, () => updateHarnessAccount(harness.id, account.id, { paused: !account.paused }))}
                />
                {!account.isDefault && (
                  <IconButton
                    title={m.settings_accounts_remove()}
                    aria-label={m.settings_accounts_remove()}
                    disabled={busy != null}
                    onClick={() => {
                      if (!window.confirm(m.settings_accounts_remove_confirm({ name: account.id }))) return;
                      void run(`remove:${account.id}`, () => removeHarnessAccount(harness.id, account.id));
                    }}
                  >
                    {busy === `remove:${account.id}` ? <Spinner /> : <Trash2 size={13} />}
                  </IconButton>
                )}
              </div>
              <ul className="mt-2 mb-0 flex list-none flex-col gap-1.5 p-0">
                {account.pools.map((pool) => {
                  const pct = pool.remaining != null ? Math.max(0, Math.min(100, Math.round(pool.remaining * 100))) : null;
                  return (
                    <li key={pool.pool} className="flex flex-wrap items-center gap-2 text-subtext">
                      <span aria-hidden="true" className={cn("size-2 shrink-0 rounded-full", QUOTA_DOT[pool.state])} />
                      <span className="shrink-0 font-medium text-text">{pool.label}</span>
                      {pct != null && (
                        <span
                          aria-hidden="true"
                          className="h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-panel"
                        >
                          <span
                            className={cn("block h-full rounded-full transition-[width] duration-150", QUOTA_DOT[pool.state])}
                            style={{ width: `${pct}%` }}
                          />
                        </span>
                      )}
                      <span className="min-w-0 flex-1 text-xs">{poolSummary(pool)}</span>
                    </li>
                  );
                })}
              </ul>
              {account.readError && (
                <p className="mt-1.5 mb-0 text-sm text-subtext">{m.settings_accounts_read_error({ error: account.readError })}</p>
              )}
              {editing === account.id && (
                <AccountLimitsForm
                  pools={group.pools}
                  limits={account.limits}
                  busy={busy != null}
                  onSave={async (limits) => {
                    if (await run(`limits:${account.id}`, () => updateHarnessAccount(harness.id, account.id, { limits }))) setEditing(null);
                  }}
                />
              )}
              {loginRun.run?.owner === account.id && (
                <CommandRunTerminal run={loginRun.run} onComplete={() => void query.refetch()} onClose={loginRun.clear} />
              )}
            </li>
          );
        })}
      </ul>
      <form
        className={cn(FORM_CLASS_NAME, "mt-3")}
        onSubmit={(event) => {
          event.preventDefault();
          void add();
        }}
      >
        <div className="flex flex-wrap items-end gap-2">
          <label className="min-w-40 flex-1">
            {m.settings_accounts_name()}
            <Input value={newId} onChange={(event) => setNewId(event.target.value)} placeholder={m.settings_accounts_name_placeholder()} autoComplete="off" spellCheck={false} />
          </label>
          <label className="min-w-40 flex-1">
            {m.settings_accounts_note()}
            <Input value={newLabel} onChange={(event) => setNewLabel(event.target.value)} autoComplete="off" />
          </label>
          <Button variant="primary" type="submit" disabled={busy != null || !newId.trim()}>
            {busy === "add" ? <Spinner /> : <Plus size={13} />} {m.settings_accounts_add()}
          </Button>
        </div>
      </form>
    </div>
  );
}

function AccountLimitsForm({ pools, limits, busy, onSave }: {
  pools: { id: string; label: string }[];
  limits: HarnessPoolLimit[];
  busy: boolean;
  onSave: (limits: HarnessPoolLimit[]) => void;
}) {
  const [draft, setDraft] = useState<Record<string, { limit: string; unit: HarnessPoolLimit["unit"]; hours: string }>>(() =>
    Object.fromEntries(pools.map((pool) => {
      const limit = limits.find((entry) => entry.pool === pool.id);
      return [pool.id, { limit: limit ? String(limit.limit) : "", unit: limit?.unit ?? "turns", hours: String(limit?.windowHours ?? 5) }];
    })),
  );
  const set = (pool: string, patch: Partial<(typeof draft)[string]>) => setDraft((current) => ({ ...current, [pool]: { ...current[pool], ...patch } }));
  return (
    <form
      className={cn(FORM_CLASS_NAME, "mt-2.5 border-t border-t-border-variant pt-2.5")}
      onSubmit={(event) => {
        event.preventDefault();
        onSave(pools.flatMap((pool) => {
          const entry = draft[pool.id];
          const limit = Number(entry.limit);
          const hours = Number(entry.hours);
          return entry.limit.trim() && limit > 0 && hours > 0 ? [{ pool: pool.id, limit, unit: entry.unit, windowHours: hours }] : [];
        }));
      }}
    >
      <p className="m-0 text-sm text-subtext">{m.settings_accounts_limit_hint()}</p>
      {pools.map((pool) => (
        <div key={pool.id} className="flex flex-wrap items-end gap-2">
          <span className="w-40 shrink-0 pb-2 text-sm text-text">{pool.label}</span>
          <label className="w-28">
            {m.settings_accounts_limit_amount()}
            <Input type="number" min={0} value={draft[pool.id].limit} onChange={(event) => set(pool.id, { limit: event.target.value })} />
          </label>
          <OptionPicker
            variant="field"
            dropDown
            title={m.settings_accounts_limit_unit()}
            choices={[
              { id: "turns", label: m.settings_accounts_unit_turns() },
              { id: "tokens", label: m.settings_accounts_unit_tokens() },
            ]}
            value={draft[pool.id].unit}
            onSelect={(id) => set(pool.id, { unit: id === "tokens" ? "tokens" : "turns" })}
          />
          <label className="w-24">
            {m.settings_accounts_limit_window()}
            <Input type="number" min={1} value={draft[pool.id].hours} onChange={(event) => set(pool.id, { hours: event.target.value })} />
          </label>
        </div>
      ))}
      <div className="actions">
        <Button variant="primary" type="submit" disabled={busy}>
          {m.settings_accounts_save_limits()}
        </Button>
      </div>
    </form>
  );
}

/** The model vendor behind a harness; `null` when it follows the chosen model. */
function harnessVendor(id: HarnessId): string | null {
  switch (id) {
    case "claude-code":
      return "Anthropic";
    case "codex":
      return "OpenAI";
    case "antigravity":
      return "Google";
    default:
      return null;
  }
}

export function CrossHarnessCard({ harnesses }: { harnesses: Harness[] | null }) {
  const options = getCrossHarnessSettingsQuery();
  const query = useQuery(options);
  const mutation = useMutation({ mutationFn: setCrossHarnessSettings });
  const [error, setError] = useState<string | null>(null);
  const enabled = query.data?.enabled ?? false;
  const excluded = query.data?.excluded ?? [];
  const readyVendors = [
    ...new Set(
      (harnesses ?? [])
        .filter((harness) => harness.agentReady && !excluded.includes(harness.id))
        .map((harness) => harnessVendor(harness.id))
        .filter((vendor): vendor is string => vendor !== null),
    ),
  ];

  async function save(update: { enabled?: boolean; excluded?: HarnessId[] }) {
    setError(null);
    try {
      const next = await mutation.mutateAsync(update);
      setScopedQueryData(options.queryKey, () => next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className={cn(SETTINGS_CARD_CLASS_NAME, "mt-4")}>
      <div className="flex items-center justify-between gap-6">
        <div>
          <div className="text-base font-medium">{m.settings_cross_harness_title()}</div>
          <p className="mt-[3px] mb-0 text-sm leading-relaxed text-text">{m.settings_cross_harness_description()}</p>
        </div>
        <Switch
          type="button"
          checked={enabled}
          aria-label={m.settings_cross_harness_title()}
          disabled={!query.data || mutation.isPending}
          onClick={() => void save({ enabled: !enabled })}
        />
      </div>
      {harnesses && (
        <>
          <ul className="mt-3 mb-0 flex flex-col gap-2 p-0 list-none">
            {harnesses.map((harness) => {
              const status = harnessStatus(harness);
              return (
                <li key={harness.id} className="flex items-center gap-2.5 text-sm">
                  <HarnessLogo harness={harness.id} />
                  <span className="font-medium">{harness.name}</span>
                  <span className="text-subtext">{harnessVendor(harness.id) ?? m.settings_cross_harness_vendor_varies()}</span>
                  <span className="flex-1" />
                  <Badge variant={status.variant}>{status.label}</Badge>
                  <Switch
                    type="button"
                    checked={!excluded.includes(harness.id)}
                    aria-label={m.settings_cross_harness_use({ harness: harness.name })}
                    disabled={!query.data || !enabled || mutation.isPending}
                    onClick={() =>
                      void save({
                        excluded: excluded.includes(harness.id)
                          ? excluded.filter((id) => id !== harness.id)
                          : [...excluded, harness.id],
                      })
                    }
                  />
                </li>
              );
            })}
          </ul>
          <p className="mt-3 mb-0 text-sm text-subtext">
            {readyVendors.length >= 2
              ? m.settings_cross_harness_vendors_ready({ vendors: readyVendors.join(", ") })
              : m.settings_cross_harness_needs_two()}
          </p>
        </>
      )}
      {error && <div className="error">{error}</div>}
    </div>
  );
}

/** Lets agents pick the backend per run among the providers left on. */
export function ComputeRoutingCard({ targets }: { targets: ComputeTargetSummary[] }) {
  const options = getComputeRoutingSettingsQuery();
  const query = useQuery(options);
  const mutation = useMutation({ mutationFn: setComputeRoutingSettings });
  const [error, setError] = useState<string | null>(null);
  const enabled = query.data?.enabled ?? false;
  const excluded = query.data?.excluded ?? [];
  const usable = targets.filter((target) => !excluded.includes(target.id));

  async function save(update: { enabled?: boolean; excluded?: ComputeTargetId[] }) {
    setError(null);
    try {
      const next = await mutation.mutateAsync(update);
      setScopedQueryData(options.queryKey, () => next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className={cn(SETTINGS_CARD_CLASS_NAME, "mb-8")}>
      <div className="flex items-center justify-between gap-6">
        <div>
          <div className="text-base font-medium">{m.settings_compute_routing_title()}</div>
          <p className="mt-[3px] mb-0 text-sm leading-relaxed text-text">{m.settings_compute_routing_description()}</p>
        </div>
        <Switch
          type="button"
          checked={enabled}
          aria-label={m.settings_compute_routing_title()}
          disabled={!query.data || mutation.isPending}
          onClick={() => void save({ enabled: !enabled })}
        />
      </div>
      {targets.length > 0 && (
        <ul className="mt-3 mb-0 flex flex-col gap-2 p-0 list-none">
          {targets.map((target) => (
            <li key={target.id} className="flex items-center gap-2.5 text-sm">
              <BackendLogo kind={TARGET_KIND[target.id]} size={18} />
              <span className="font-medium">{TARGET_LABELS[target.id]()}</span>
              <span className="min-w-0 flex-1 truncate text-subtext">{target.summary}</span>
              <Switch
                type="button"
                checked={!excluded.includes(target.id)}
                aria-label={m.settings_compute_routing_use({ backend: TARGET_LABELS[target.id]() })}
                disabled={!query.data || !enabled || mutation.isPending}
                onClick={() =>
                  void save({
                    excluded: excluded.includes(target.id)
                      ? excluded.filter((id) => id !== target.id)
                      : [...excluded, target.id],
                  })
                }
              />
            </li>
          ))}
        </ul>
      )}
      {enabled && usable.length < 2 && <p className="mt-3 mb-0 text-sm text-subtext">{m.settings_compute_routing_needs_two()}</p>}
      {error && <div className="error">{error}</div>}
    </div>
  );
}

/** Thresholds offered for auto-compact, in percent of the context window. */
const AUTO_COMPACT_PERCENTS = [60, 70, 75, 80, 85, 90, 95];
/** Token limits offered for auto-compact; `0` is no limit. */
const AUTO_COMPACT_TOKENS = [100_000, 150_000, 200_000, 300_000, 500_000, 0];

/** Tokens at which a `window`-token chat auto-compacts; `maxTokens` 0 is no limit. */
function autoCompactLimit(window: number, percent: number, maxTokens: number): number {
  const byPercent = (window * percent) / 100;
  return maxTokens ? Math.min(byPercent, maxTokens) : byPercent;
}

/** A number the user types, saved on Enter or when focus leaves; Escape
 * restores the saved value. `suggestions` fill the browser's dropdown. */
function NumberField({ id, value, min, max, placeholder, suggestions, disabled, onCommit }: {
  id: string;
  value: string;
  min?: number;
  max?: number;
  placeholder?: string;
  suggestions: number[];
  disabled: boolean;
  onCommit: (text: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const commit = () => { if (draft !== value) onCommit(draft); };
  return (
    <>
      <Input
        id={id}
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        list={`${id}-suggestions`}
        placeholder={placeholder}
        value={draft}
        disabled={disabled}
        className="h-9 tabular-nums"
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") { event.preventDefault(); commit(); }
          else if (event.key === "Escape") setDraft(value);
        }}
      />
      <datalist id={`${id}-suggestions`}>
        {suggestions.map((suggestion) => <option key={suggestion} value={suggestion} />)}
      </datalist>
    </>
  );
}

/** Compacts a chat on its own once a turn ends with the context this full. */
export function AutoCompactCard() {
  const options = getAutoCompactSettingsQuery();
  const query = useQuery(options);
  const mutation = useMutation({ mutationFn: setAutoCompactSettings });
  const [error, setError] = useState<string | null>(null);
  const enabled = query.data?.enabled ?? true;
  const percent = query.data?.percent ?? 80;
  const maxTokens = query.data ? query.data.maxTokens ?? 0 : 200_000;

  async function save(update: { enabled?: boolean; percent?: number; maxTokens?: number }) {
    setError(null);
    try {
      const next = await mutation.mutateAsync(update);
      setScopedQueryData(options.queryKey, () => next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className={cn(SETTINGS_CARD_CLASS_NAME, "mb-8")}>
      <div className="flex items-center justify-between gap-6">
        <div>
          <div className="text-base font-medium">{m.settings_auto_compact_title()}</div>
          <p className="mt-[3px] mb-0 text-sm leading-relaxed text-text">{m.settings_auto_compact_description()}</p>
        </div>
        <Switch
          type="button"
          checked={enabled}
          aria-label={m.settings_auto_compact_title()}
          disabled={!query.data || mutation.isPending}
          onClick={() => void save({ enabled: !enabled })}
        />
      </div>
      <div className="mt-3 grid grid-cols-[auto_minmax(0,9rem)_auto] items-center gap-x-2.5 gap-y-2 text-sm">
        <label htmlFor="auto-compact-percent">{m.settings_auto_compact_threshold()}</label>
        <NumberField
          id="auto-compact-percent"
          key={`percent-${percent}`}
          value={String(percent)}
          min={50}
          max={95}
          suggestions={AUTO_COMPACT_PERCENTS}
          disabled={!query.data || !enabled || mutation.isPending}
          onCommit={(text) => {
            const value = Number(text);
            if (!Number.isInteger(value) || value < 50 || value > 95) return setError(m.settings_auto_compact_percent_invalid());
            if (value !== percent) void save({ percent: value });
          }}
        />
        <span className="text-subtext">%</span>
        <label htmlFor="auto-compact-tokens">{m.settings_auto_compact_max_tokens()}</label>
        <NumberField
          id="auto-compact-tokens"
          key={`tokens-${maxTokens}`}
          value={maxTokens ? String(maxTokens / 1000) : ""}
          min={50}
          placeholder={m.settings_auto_compact_no_limit()}
          suggestions={AUTO_COMPACT_TOKENS.filter(Boolean).map((tokens) => tokens / 1000)}
          disabled={!query.data || !enabled || mutation.isPending}
          onCommit={(text) => {
            const value = text.trim() === "" ? 0 : Math.round(Number(text) * 1000);
            if (!Number.isFinite(value) || (value !== 0 && value < 50_000)) return setError(m.settings_auto_compact_tokens_invalid());
            if (value !== maxTokens) void save({ maxTokens: value });
          }}
        />
        <span className="text-subtext">{m.settings_auto_compact_tokens_unit()}</span>
      </div>
      <p className="mt-2 mb-0 text-sm leading-relaxed text-subtext">{m.settings_auto_compact_input_hint()}</p>
      <p className="mt-2 mb-0 text-sm leading-relaxed text-subtext">{m.settings_auto_compact_cost_hint()}</p>
      <div className="mt-4 pt-3.5 border-t border-t-border-variant">
        <div className="text-sm font-medium">{m.settings_auto_compact_how_title()}</div>
        <ul className="mt-1.5 mb-0 ps-5 list-disc flex flex-col gap-1.5 text-sm leading-relaxed text-text">
          <li>
            {m.settings_auto_compact_how_when({
              percent: `${percent}%`,
              tokens: maxTokens ? fmtTokens(maxTokens) : "—",
            })}
          </li>
          <li>
            {m.settings_auto_compact_how_examples({
              small: fmtTokens(autoCompactLimit(200_000, percent, maxTokens)),
              large: fmtTokens(autoCompactLimit(1_000_000, percent, maxTokens)),
            })}
          </li>
          <li>{m.settings_auto_compact_how_method()}</li>
          <li>{m.settings_auto_compact_how_kept()}</li>
          <li>{m.settings_auto_compact_how_skip()}</li>
          <li>{m.settings_auto_compact_how_manual()}</li>
        </ul>
      </div>
      {error && <div className="error">{error}</div>}
    </div>
  );
}

export const fmtUsd = (value: number) =>
  value.toLocaleString(undefined, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 });

/** How many of the cheapest GPU offers show before "Show all". */
const PRICE_ROWS = 8;

/** Remaining credit and GPU prices on every connected provider: the same
 * numbers agents read with `orx compute prices` before choosing a backend. */
export function ProviderPricesCard() {
  const options = getComputePricesQuery();
  const query = useQuery(options);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const providers = (query.data?.providers ?? []).filter((provider) => provider.configured);
  const paid = providers.filter((provider) => provider.billing === "credits" || provider.billing === "usage");
  const own = providers.filter((provider) => provider.billing === "own");
  const offers = paid
    .flatMap((provider) =>
      provider.offers
        .filter((offer) => offer.gpu && offer.usdPerHour != null)
        .map((offer) => ({ provider: provider.id, offer })),
    )
    .sort((a, b) => (a.offer.usdPerHour ?? 0) - (b.offer.usdPerHour ?? 0));
  const shown = showAll ? offers : offers.slice(0, PRICE_ROWS);
  const anyRunway = offers.some(({ offer }) => offer.runwayHours != null);

  async function refresh() {
    if (refreshing) return;
    setRefreshing(true);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await getComputePrices(undefined, true));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }

  return (
    <div className={cn(SETTINGS_CARD_CLASS_NAME, "provider-prices mb-8")}>
      <div className="flex items-start justify-between gap-6">
        <div>
          <div className="text-base font-medium">{m.settings_prices_title()}</div>
          <p className="mt-[3px] mb-0 text-sm leading-relaxed text-text">{m.settings_prices_description()}</p>
        </div>
        <Button size="small" onClick={() => void refresh()} disabled={query.isPending || refreshing}>
          <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
        </Button>
      </div>
      {query.isPending ? (
        <LoadingRow>
          <Spinner /> {m.common_checking()}
        </LoadingRow>
      ) : (
        <>
          {paid.length > 0 && (
            <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {paid.map((provider) => (
                <div key={provider.id} className="rounded-lg border border-border bg-surface px-4 py-3">
                  <div className="flex items-center gap-2 text-sm text-subtext">
                    <BackendLogo kind={TARGET_KIND[provider.id]} size={16} />
                    {TARGET_LABELS[provider.id]()}
                  </div>
                  <div className="mt-1 text-xl font-semibold tabular-nums text-text">
                    {provider.balance
                      ? provider.balance.unit === "USD"
                        ? m.settings_prices_available({ amount: fmtUsd(provider.balance.amount) })
                        : `${fmtUnits(provider.balance.amount)} ${provider.balance.unit}`
                      : provider.spend?.meteredUsd != null
                        ? m.settings_prices_used_month({ amount: fmtUsd(provider.spend.meteredUsd) })
                        : provider.billing === "credits"
                          ? m.settings_prices_balance_unknown()
                          : m.settings_prices_pay_as_you_go()}
                  </div>
                  {provider.balance?.usd != null && provider.balance.unit !== "USD" && (
                    <div className="mt-0.5 text-sm text-subtext tabular-nums">≈ {fmtUsd(provider.balance.usd)}</div>
                  )}
                  {provider.spend && (
                    <div className="mt-0.5 text-sm text-subtext tabular-nums">
                      {provider.spend.billedUsd != null && m.settings_prices_billed({ amount: fmtUsd(provider.spend.billedUsd) })}
                      {provider.spend.creditsUsd != null && provider.spend.creditsUsd > 0 && (
                        <> · {m.settings_prices_credits_applied({ amount: fmtUsd(provider.spend.creditsUsd) })}</>
                      )}
                    </div>
                  )}
                  {provider.error ? (
                    <div className="mt-1 text-sm text-subtext">{provider.error}</div>
                  ) : provider.note && (
                    <div className="mt-1 text-sm leading-snug text-subtext">{provider.note}</div>
                  )}
                </div>
              ))}
            </div>
          )}
          {offers.length > 0 ? (
            <div className="mt-4 overflow-x-auto">
              <table className="provider-offers w-full border-collapse text-sm">
                <thead>
                  <tr className="text-subtext [&_th]:border-b [&_th]:border-border [&_th]:pb-1.5 [&_th]:font-medium">
                    <th className="text-start">{m.settings_prices_col_gpu()}</th>
                    <th className="text-start">{m.settings_prices_col_provider()}</th>
                    <th className="text-end">{m.settings_colab_col_memory()}</th>
                    <th className="text-end">{m.settings_prices_col_price()}</th>
                    {anyRunway && <th className="text-end">{m.settings_colab_col_hours()}</th>}
                    <th className="text-end">{m.settings_prices_col_flavor()}</th>
                  </tr>
                </thead>
                <tbody>
                  {shown.map(({ provider, offer }) => (
                    <tr
                      key={`${provider}:${offer.flavor}`}
                      className={cn(
                        "transition-colors hover:bg-surface [&_td]:border-b [&_td]:border-border [&_td]:py-1.5",
                        offer.available === false && "text-subtext",
                      )}
                    >
                      <td className="font-medium">
                        {offer.gpu}
                        {offer.gpuCount > 1 && <span className="ms-1 text-subtext">×{offer.gpuCount}</span>}
                      </td>
                      <td>
                        <span className="inline-flex items-center gap-1.5">
                          <BackendLogo kind={TARGET_KIND[provider]} size={14} />
                          {TARGET_LABELS[provider]()}
                          {offer.host && <span className="text-subtext">· {offer.host}</span>}
                        </span>
                      </td>
                      <td className="text-end tabular-nums">
                        {offer.vramGb != null ? m.settings_colab_memory_gb({ gb: fmtNumber(offer.vramGb) }) : "—"}
                      </td>
                      <td className="text-end tabular-nums" title={offer.estimated ? m.settings_prices_estimated_note() : undefined}>
                        {offer.estimated ? "≈ " : ""}
                        {m.settings_prices_per_hour({ price: fmtUsd(offer.usdPerHour ?? 0) })}
                        {offer.unitsPerHour != null && (
                          <span className="ms-1 text-subtext">({fmtUnits(offer.unitsPerHour)} CU)</span>
                        )}
                      </td>
                      {anyRunway && (
                        <td className="text-end tabular-nums">
                          {offer.available === false
                            ? m.settings_prices_not_on_plan()
                            : offer.runwayHours != null
                              ? m.settings_colab_hours_value({ hours: offer.runwayHours.toLocaleString(undefined, { maximumFractionDigits: 1 }) })
                              : "—"}
                        </td>
                      )}
                      <td className="text-end"><code>{offer.flavor}</code></td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <div className="mt-2 flex flex-wrap items-center gap-3">
                <p className="m-0 flex-1 text-sm text-subtext">{m.settings_prices_estimated_note()}</p>
                {offers.length > PRICE_ROWS && (
                  <Button size="small" variant="ghost" onClick={() => setShowAll(!showAll)}>
                    {showAll ? m.settings_prices_show_less() : m.settings_prices_show_all({ count: fmtNumber(offers.length) })}
                  </Button>
                )}
              </div>
            </div>
          ) : (
            <p className="mt-3 mb-0 text-sm text-subtext">{m.settings_prices_none()}</p>
          )}
          {own.length > 0 && (
            <p className="mt-3 mb-0 text-sm text-subtext">
              {m.settings_prices_own_hardware({ backends: own.map((provider) => TARGET_LABELS[provider.id]()).join(", ") })}
            </p>
          )}
        </>
      )}
      {error && <div className="error">{error}</div>}
    </div>
  );
}

/** Google Cloud: gcloud sign-in, project and zone, Spot VMs, the GPUs a run
 * can ask for, and the VMs orx created that still exist. */
export function GcpSection() {
  const options = getGcpSettingsQuery();
  const query = useQuery(options);
  const data: GcpSettings | null = query.data ?? null;
  const [project, setProject] = useState("");
  const [zone, setZone] = useState("");
  const [credit, setCredit] = useState("");
  const [exportTable, setExportTable] = useState("");
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!data) return;
    setProject(data.settings.project ?? "");
    setZone(data.settings.zone ?? "");
    setExportTable(data.settings.billingExportTable ?? "");
    setCredit("");
  }, [data]);

  async function refresh() {
    setRefreshing(true);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await getGcpSettings());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }

  async function save(update: GcpSettingsUpdate) {
    setSaving(true);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await saveGcpSettings(update));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function deleteInstance(zone: string, name: string) {
    if (!window.confirm(m.settings_gcp_delete_confirm({ name }))) return;
    setDeleting(name);
    setError(null);
    try {
      await stopGcpInstance(zone, name);
      queryClient.setQueryData(options.queryKey, await getGcpSettings());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  }

  if (query.isPending) {
    return (
      <LoadingRow>
        <Spinner /> {m.common_checking()}
      </LoadingRow>
    );
  }
  if (!data) return <div className="error">{query.error?.message ?? error}</div>;
  const { preflight } = data;
  const unchanged = project === (data.settings.project ?? "") && zone === (data.settings.zone ?? "");
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={data.ready ? "success" : "warning"}>
          {data.ready
            ? m.settings_page_ready_to_use()
            : !preflight.cliPath
              ? m.settings_gcp_not_installed()
              : !preflight.account
                ? m.settings_gcp_not_signed_in()
                : m.settings_gcp_not_ready()}
        </Badge>
        {preflight.account && <span className="truncate text-sm text-subtext">{preflight.account}</span>}
        <div className="flex-1" />
        <Button size="small" onClick={() => void refresh()} disabled={refreshing}>
          <RefreshCw size={12} className={refreshing ? "animate-[spin_0.9s_linear_infinite]" : ""} /> {m.settings_page_refresh()}
        </Button>
      </div>
      {preflight.error && <p className={COMPUTE_DIAGNOSTIC_CLASS_NAME}>{preflight.error}</p>}
      {data.ready && (
        <div className="mt-4 rounded-lg border border-border bg-surface px-4 py-3">
          <div className="text-sm text-subtext">{m.settings_gcp_available()}</div>
          <div className={cn("mt-1 text-2xl font-semibold tabular-nums", data.balance && data.balance.availableUsd <= 0 ? "text-accent-red" : "text-text")}>
            {data.balance ? `≈ ${fmtUsd(data.balance.availableUsd)}` : "—"}
          </div>
          {data.balance ? (
            <div className="mt-0.5 text-sm text-subtext">
              {m.settings_gcp_balance_detail({
                credit: fmtUsd(data.balance.creditUsd),
                date: new Date(data.balance.asOf).toLocaleDateString(),
                spent: fmtUsd(data.balance.spentUsd),
              })}{" "}
              {data.balance.source === "export" ? m.settings_gcp_source_export() : m.settings_gcp_source_estimate()}
            </div>
          ) : (
            <div className="mt-0.5 text-sm text-subtext">{m.settings_gcp_no_credit()}</div>
          )}
          {data.balance?.error && <div className="mt-1 text-sm text-subtext">{data.balance.error}</div>}
          {data.billing && (
            <div className="mt-1 text-sm text-subtext">
              {data.billing.enabled
                ? m.settings_gcp_billing_account({ name: data.billing.name ?? data.billing.id })
                : m.settings_gcp_billing_disabled()}
            </div>
          )}
          <form
            className="mt-3 flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              const amount = Number(credit);
              if (credit.trim() && Number.isFinite(amount) && amount >= 0) void save({ creditUsd: amount });
            }}
          >
            <label className="flex min-w-48 flex-1 flex-col gap-1 text-sm">
              {m.settings_gcp_credit_label()}
              <Input
                type="number"
                min="0"
                step="0.01"
                inputMode="decimal"
                value={credit}
                onChange={(event) => setCredit(event.target.value)}
                placeholder={data.settings.creditUsd != null ? String(data.settings.creditUsd) : "300.00"}
              />
            </label>
            <Button type="submit" disabled={saving || !credit.trim()}>
              {m.common_save()}
            </Button>
            {data.settings.creditUsd != null && (
              <Button type="button" variant="ghost" disabled={saving} onClick={() => void save({ clearCredit: true })}>
                {m.settings_gcp_clear_credit()}
              </Button>
            )}
          </form>
          <p className="mt-1.5 mb-0 text-sm text-subtext">{m.settings_gcp_credit_hint()}</p>
          <form
            className="mt-3 flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void save({ billingExportTable: exportTable });
            }}
          >
            <label className="flex min-w-48 flex-1 flex-col gap-1 text-sm">
              {m.settings_gcp_export_label()}
              <Input
                type="text"
                value={exportTable}
                onChange={(event) => setExportTable(event.target.value)}
                placeholder="my-project.billing.gcp_billing_export_v1_XXXX"
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <Button type="submit" disabled={saving || exportTable === (data.settings.billingExportTable ?? "")}>
              {m.common_save()}
            </Button>
          </form>
          <p className="mt-1.5 mb-0 text-sm text-subtext">{m.settings_gcp_export_hint()}</p>
        </div>
      )}
      {!preflight.cliPath && (
        <p className="mt-3 mb-0 text-sm text-subtext">
          {m.settings_gcp_install_hint()} <code>gcloud auth login</code>
        </p>
      )}
      <form
        className={FORM_CLASS_NAME}
        onSubmit={(event) => {
          event.preventDefault();
          void save({ project, zone });
        }}
      >
        <label>
          {m.settings_gcp_project()}
          <Input
            type="text"
            value={project}
            onChange={(event) => setProject(event.target.value)}
            placeholder={preflight.project ?? "my-project-id"}
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <label>
          {m.settings_gcp_zone()}
          <Input
            type="text"
            value={zone}
            onChange={(event) => setZone(event.target.value)}
            placeholder={preflight.zone}
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <p className="m-0 text-sm text-subtext">{m.settings_gcp_location_hint()}</p>
        <div className="actions">
          <Button variant="primary" type="submit" disabled={saving || unchanged}>
            {saving ? m.common_saving() : m.common_save()}
          </Button>
        </div>
      </form>
      <div className="mt-4 flex items-center justify-between gap-6">
        <div>
          <div className="text-sm font-medium">{m.settings_gcp_spot()}</div>
          <p className="mt-0.5 mb-0 text-sm text-subtext">{m.settings_gcp_spot_hint()}</p>
        </div>
        <Switch
          type="button"
          checked={data.settings.spot}
          aria-label={m.settings_gcp_spot()}
          disabled={saving}
          onClick={() => void save({ spot: !data.settings.spot })}
        />
      </div>
      {data.instances && (
        <div className="mt-4">
          <p className="mt-0 mb-2 text-sm font-medium text-subtext">{m.settings_gcp_instances()}</p>
          {data.instances.length === 0 ? (
            <p className="m-0 text-sm text-subtext">{m.settings_gcp_no_instances()}</p>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-1.5 p-0 text-sm">
              {data.instances.map((instance) => (
                <li key={instance.name} className="flex items-center gap-3 rounded-md border border-border px-3 py-2">
                  <code>{instance.machineType}</code>
                  <span className="min-w-0 flex-1 truncate text-subtext">
                    {instance.zone} · {instance.status}
                    {instance.runId && <> · {m.settings_gcp_instance_run({ run: instance.runId.slice(0, 8) })}</>}
                  </span>
                  <Button
                    size="small"
                    disabled={deleting === instance.name}
                    onClick={() => void deleteInstance(instance.zone, instance.name)}
                  >
                    {deleting === instance.name ? <Spinner /> : <Square size={12} />} {m.settings_gcp_delete()}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_gcp_auto_delete()}</p>
        </div>
      )}
      <div className="mt-4 overflow-x-auto">
        <table className="gcp-gpus w-full border-collapse text-sm">
          <thead>
            <tr className="text-subtext [&_th]:border-b [&_th]:border-border [&_th]:pb-1.5 [&_th]:font-medium">
              <th className="text-start">{m.settings_prices_col_flavor()}</th>
              <th className="text-start">{m.settings_prices_col_gpu()}</th>
              <th className="text-end">{m.settings_colab_col_memory()}</th>
              <th className="text-end">{m.settings_prices_col_price()}</th>
            </tr>
          </thead>
          <tbody>
            {[...data.gpus].sort((a, b) => a.usdPerHour - b.usdPerHour).map((gpu) => (
              <tr key={gpu.id} className="[&_td]:border-b [&_td]:border-border [&_td]:py-1.5">
                <td><code>{gpu.id}</code></td>
                <td>{gpu.label}</td>
                <td className="text-end tabular-nums">{m.settings_colab_memory_gb({ gb: fmtNumber(gpu.vramGb) })}</td>
                <td className="text-end tabular-nums">≈ {m.settings_prices_per_hour({ price: fmtUsd(gpu.usdPerHour) })}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_gcp_prices_note()}</p>
      </div>
      {error && <div className="error mt-2.5">{error}</div>}
    </>
  );
}

/** The opt-in fallback list: which harnesses a chat moves to, and in what
 * order, when its own harness runs out of usage. */
/** An OpenAI model that picks the account, fallback harness, and compute
 * for work, so the harnesses' own quota lasts longer. */
export function DecisionModelCard() {
  const options = getDecisionModelSettingsQuery();
  const query = useQuery(options);
  const settings = query.data ?? null;
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(patch: { enabled?: boolean; model?: string; apiKey?: string }) {
    setSaving(true);
    setError(null);
    try {
      queryClient.setQueryData(options.queryKey, await saveDecisionModelSettings(patch));
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setSaving(false);
    }
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving) return;
    const patch: { model?: string; apiKey?: string } = {};
    if (apiKey.trim()) patch.apiKey = apiKey.trim();
    if (model != null && model.trim() !== settings?.model) patch.model = model.trim();
    if (!patch.apiKey && patch.model == null) return;
    if (await save(patch)) {
      setApiKey("");
      setModel(null);
    }
  }

  if (!settings) {
    return query.isPending ? (
      <LoadingRow>
        <Spinner /> {m.common_checking()}
      </LoadingRow>
    ) : null;
  }
  const status = settings.validationStatus;
  return (
    <div className={SETTINGS_CARD_CLASS_NAME}>
      <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
        <div>
          <div className="project-default-title text-base font-medium">{m.settings_decision_title()}</div>
          <p>{m.settings_decision_description()}</p>
        </div>
        <Switch
          type="button"
          checked={settings.enabled}
          aria-label={m.settings_decision_title()}
          disabled={saving}
          onClick={() => void save({ enabled: !settings.enabled })}
        />
      </div>
      <dl className="mt-3 mb-0 flex items-center justify-between gap-4 text-sm">
        <dt className="font-medium text-subtext">{m.settings_page_status()}</dt>
        <dd className="m-0">
          <DecisionStatusBadge settings={settings} />
        </dd>
      </dl>
      {settings.enabled && !settings.keySource && <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_decision_needs_key()}</p>}
      {status === "modelUnavailable" && <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_decision_model_unavailable({ model: settings.model })}</p>}
      {settings.keySource === "env" && <p className="mt-2 mb-0 text-sm text-subtext">{m.settings_decision_env_override()}</p>}
      <form className="mt-4 flex flex-col gap-4" onSubmit={submit}>
        <label className="flex flex-col gap-2 text-sm font-medium text-subtext">
          {settings.maskedKey ? m.settings_replace_key() : m.settings_decision_api_key()}
          <Input
            type="password"
            value={apiKey}
            placeholder={settings.maskedKey ?? "sk-…"}
            onChange={(e) => setApiKey(e.target.value)}
            autoComplete="new-password"
          />
        </label>
        <label className="flex flex-col gap-2 text-sm font-medium text-subtext">
          {m.settings_decision_model()}
          <Input
            value={model ?? settings.model}
            placeholder={settings.defaultModel}
            onChange={(e) => setModel(e.target.value)}
            spellCheck={false}
          />
          <span className="font-normal">{m.settings_decision_model_hint({ model: settings.defaultModel })}</span>
        </label>
        {error && <p className="m-0 text-sm text-accent-red">{error}</p>}
        <div className="flex justify-end gap-2">
          {settings.maskedKey && settings.keySource === "settings" && (
            <Button
              type="button"
              disabled={saving}
              onClick={() => {
                if (window.confirm(m.settings_decision_remove_key_confirm())) void save({ apiKey: "" });
              }}
            >
              {m.settings_decision_remove_key()}
            </Button>
          )}
          <Button variant="primary" type="submit" disabled={saving || (!apiKey.trim() && (model == null || model.trim() === settings.model))}>
            {saving ? m.settings_validating() : m.common_save()}
          </Button>
        </div>
      </form>
    </div>
  );
}

function DecisionStatusBadge({ settings }: { settings: DecisionModelSettings }) {
  if (!settings.keySource) return <Badge variant="warning">{m.settings_page_not_configured()}</Badge>;
  if (settings.validationStatus === "invalid") return <Badge variant="error">{m.settings_decision_invalid_key()}</Badge>;
  if (settings.validationStatus === "modelUnavailable") return <Badge variant="warning">{m.settings_decision_model_blocked()}</Badge>;
  if (!settings.enabled) return <Badge>{m.settings_decision_off()}</Badge>;
  return <Badge variant="success">{m.settings_page_ready()}</Badge>;
}

export function HarnessFailoverCard({ harnesses }: { harnesses: Harness[] }) {
  const uiStateOptions = getUiStateQuery();
  const { data: uiState } = useQuery(uiStateOptions);
  const save = useMutation({
    mutationFn: (harnessFailover: HarnessFailover) => updateUiState({ harnessFailover }),
    onSuccess: (state) => setScopedQueryData(uiStateOptions.queryKey, state),
    onError: (error) => showAlert(error instanceof Error ? error.message : String(error), "error"),
  });
  const stored = save.isPending ? save.variables : uiState?.harnessFailover;
  if (!stored) return null;
  // An empty order means "every harness": show them all switched on.
  const order = stored.order.length > 0 ? stored.order : harnesses.map((harness) => harness.id);
  const rows = [
    ...order.flatMap((id) => harnesses.filter((harness) => harness.id === id)),
    ...harnesses.filter((harness) => !order.includes(harness.id)),
  ];
  const update = (next: Partial<HarnessFailover>) => save.mutate({ enabled: stored.enabled, order, ...next });
  const move = (index: number, by: number) => {
    const next = [...order];
    const [id] = next.splice(index, 1);
    next.splice(index + by, 0, id);
    update({ order: next });
  };
  return (
    <div className={SETTINGS_CARD_CLASS_NAME}>
      <div className={PROJECT_DEFAULT_ROW_CLASS_NAME}>
        <div>
          <div className="project-default-title text-base font-medium">{m.settings_failover_title()}</div>
          <p>{m.settings_failover_description()}</p>
        </div>
        <Switch
          type="button"
          checked={stored.enabled}
          aria-label={m.settings_failover_title()}
          disabled={save.isPending}
          onClick={() => update({ enabled: !stored.enabled })}
        />
      </div>
      {stored.enabled && (
        <div className="mt-3.5 pt-3.5 border-t border-t-border-variant">
          <div className="text-sm text-subtext mb-2">{m.settings_failover_order()}</div>
          <ol className="flex flex-col gap-1">
            {rows.map((harness) => {
              const index = order.indexOf(harness.id);
              const included = index >= 0;
              const status = harnessStatus(harness);
              return (
                <li key={harness.id} className="flex items-center gap-2.5 min-w-0">
                  <span className="w-5 text-end text-sm text-muted tabular-nums">{included ? fmtNumber(index + 1) : ""}</span>
                  <HarnessLogo harness={harness.id} />
                  <span className={cn("flex-1 min-w-0 truncate text-base", !included && "text-subtext")}>{harness.name}</span>
                  <span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full bg-muted [&.ok]:bg-accent-green [&.warn]:bg-accent-amber ${status.cls}`} />
                  <span className="sr-only">{status.label}</span>
                  <IconButton
                    size="small"
                    title={m.settings_failover_move_up({ name: ltr(harness.name) })}
                    aria-label={m.settings_failover_move_up({ name: ltr(harness.name) })}
                    disabled={!included || index === 0 || save.isPending}
                    onClick={() => move(index, -1)}
                  >
                    <ArrowUp size={14} />
                  </IconButton>
                  <IconButton
                    size="small"
                    title={m.settings_failover_move_down({ name: ltr(harness.name) })}
                    aria-label={m.settings_failover_move_down({ name: ltr(harness.name) })}
                    disabled={!included || index === order.length - 1 || save.isPending}
                    onClick={() => move(index, 1)}
                  >
                    <ArrowDown size={14} />
                  </IconButton>
                  <Switch
                    type="button"
                    checked={included}
                    aria-label={m.settings_failover_include({ name: ltr(harness.name) })}
                    // Turning off the last one would read back as "all of them".
                    disabled={save.isPending || (included && order.length === 1)}
                    onClick={() => update({ order: included ? order.filter((id) => id !== harness.id) : [...order, harness.id] })}
                  />
                </li>
              );
            })}
          </ol>
        </div>
      )}
    </div>
  );
}

