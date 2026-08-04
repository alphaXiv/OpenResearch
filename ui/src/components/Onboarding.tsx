import { ArrowLeft, ArrowRight, Check, Copy, RefreshCw } from "lucide-react";
import { Wordmark } from "./Wordmark";
import { useEffect, useRef, useState } from "react";
import {
  getGitSettings,
  getHarnesses,
  getTelemetry,
  harnessModelLabel,
  recordTelemetryConsent,
  setTelemetry,
  type GitSettings,
  type Harness,
  type TelemetrySettings,
} from "../api";
import { GitTokenForm } from "./GitTokenForm";
import { renderNote } from "./agentNote";
import { onHarnessAuth } from "../events";

const RETRY_COPY = "Couldn't reach orx. Check it's still running, then re-check.";

/** First-run walkthrough: the detected coding agents, then GitHub access, then
 * the usage-analytics choice, then hand off to the (empty) projects page.
 * Step 1 gates — orx can't chat without a signed-in agent. Step 2 doesn't:
 * cloning and pushing work over SSH keys, so a missing token is a hint, not a
 * wall. The data-dir choice deliberately lives in Settings → Storage instead:
 * the default suits almost everyone, and Settings can also *move* existing
 * data, which this flow never could. */
export function Onboarding({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<0 | 1 | 2>(0);
  const [harnesses, setHarnesses] = useState<Harness[] | null>(null);
  const [git, setGit] = useState<GitSettings | null>(null);
  const [telemetry, setTelemetryState] = useState<TelemetrySettings | null>(null);
  const [telemetrySaving, setTelemetrySaving] = useState(false);
  const [checking, setChecking] = useState(false);
  // Per-probe, not one shared flag: a telemetry failure must not put a
  // connectivity error on a gate it has nothing to do with — or worse, hide
  // the actionable "sign in" hint behind it.
  const [harnessError, setHarnessError] = useState(false);
  const [gitError, setGitError] = useState(false);

  // Step 1 requires one genuinely usable harness. A failed or inconclusive
  // detection never bypasses the gate; the user can re-check without being
  // tricked into a chat configuration that cannot run.
  const anyAgentReady = harnesses?.some((h) => h.agentReady) ?? false;

  // Drives the nudge on step 2 only — that step doesn't gate, so an unknown
  // answer costs nothing.
  const githubConnected = git?.githubTokenSource != null;

  // Drops a slow probe whose answer a newer load — or a token save — has
  // already superseded, so a Save landing mid-refresh isn't overwritten by the
  // pre-save snapshot.
  const loadSeq = useRef(0);
  const load = (refresh: boolean, retryRejected = false) => {
    const seq = ++loadSeq.current;
    setChecking(true);
    setHarnessError(false);
    setGitError(false);
    const fresh = () => seq === loadSeq.current;
    void Promise.allSettled([
      getHarnesses(refresh, retryRejected).then((h) => fresh() && setHarnesses(h)),
      getGitSettings().then((g) => fresh() && setGit(g)),
      getTelemetry().then((t) => fresh() && setTelemetryState(t)),
    ])
      .then(([harness, git]) => {
        if (!fresh()) return;
        // Clear the stale answer too, so "errored", "loading" and "loaded"
        // stay mutually exclusive — otherwise a failed re-check leaves old
        // cards on screen saying "not signed in" while the gate un-gates.
        if (harness.status === "rejected") {
          setHarnessError(true);
          setHarnesses(null);
        }
        if (git.status === "rejected") {
          setGitError(true);
          setGit(null);
        }
      })
      // Not sequence-guarded: this is "a load is running", not data a stale
      // response could corrupt. Guarding it meant a token save (which bumps
      // loadSeq to supersede the probe) left `checking` true forever, with
      // both Re-check buttons dead for the rest of the session.
      .finally(() => setChecking(false));
  };
  useEffect(() => load(false), []);
  useEffect(
    () =>
      onHarnessAuth(() => {
        void getHarnesses(true)
          .then((next) => {
            setHarnesses(next);
            setHarnessError(false);
          })
          .catch(() => setHarnessError(true));
      }),
    [],
  );

  // Leaving step 3 → record the final consent decision once (agree or reject),
  // so every user who reaches the analytics step is counted, including those who
  // accept the default. Default to enabled if the setting hasn't loaded yet
  // (that's the default state shown). Best-effort — never block finishing.
  const finishOnboarding = () => {
    void recordTelemetryConsent(telemetry?.enabled ?? true).catch(() => {});
    onDone();
  };

  return (
    <div className="home onboarding">
      <div className="home-inner">
        {step === 0 ? (
          <>
            <div className="onb-eyebrow">
              <Wordmark /> · Step 1 of 3
            </div>
            <h2 className="onb-title">Your coding agents</h2>
            <p className="onb-sub">
              orx found the agent CLIs on this machine and drives them directly — chat and
              autoresearch run on your own logins, no extra API keys.
            </p>
            <div className="onb-cards">
              {harnesses !== null ? (
                harnesses.map((h) => <AgentCard key={h.id} h={h} />)
              ) : harnessError ? (
                // Never a spinner next to an error — detection isn't running.
                <div className="onb-card-meta">{RETRY_COPY}</div>
              ) : (
                <div className="onb-loading">
                  <span className="spinner" /> Detecting Claude Code, Codex, OpenCode…
                </div>
              )}
            </div>
            {harnesses !== null && !anyAgentReady && (
              <p className="onb-gate-hint">Sign in to at least one agent to continue.</p>
            )}
            <div className="onb-actions">
              <button className="btn ghost" onClick={() => load(true, true)} disabled={checking}>
                <RefreshCw size={12} className={checking ? "spin" : ""} /> Re-check
              </button>
              <div style={{ flex: 1 }} />
              <button
                className="btn primary"
                onClick={() => setStep(1)}
                disabled={!anyAgentReady}
                title={
                  anyAgentReady ? undefined : "Sign in to at least one coding agent to continue"
                }
              >
                Continue <ArrowRight size={13} />
              </button>
            </div>
          </>
        ) : step === 1 ? (
          <>
            <div className="onb-eyebrow">
              <Wordmark /> · Step 2 of 3
            </div>
            <h2 className="onb-title">Connect GitHub</h2>
            <p className="onb-sub">
              orx clones your GitHub repos and pushes each experiment as a branch.
            </p>
            <div className="onb-cards">
              {gitError ? (
                <div className="onb-card-meta">{RETRY_COPY}</div>
              ) : (
                <GitCard
                  git={git}
                  onUpdate={(g) => {
                    // A save is newer than any probe still in flight.
                    loadSeq.current++;
                    setGit(g);
                    setGitError(false);
                  }}
                />
              )}
            </div>
            {/* Soft, and honestly so: cloning and pushing work over SSH keys
                (ensure_clone tries ssh first), so a token is a convenience,
                not a requirement. State what's missing; never block on it. A
                disabled Continue beside an enabled Skip just reads as a bug. */}
            {git !== null && !githubConnected && (
              <p className="onb-gate-hint">
                Without GitHub access, orx can&apos;t create repos for you — cloning and pushing
                still work if you have SSH keys.
              </p>
            )}
            <div className="onb-actions">
              <button className="btn ghost" onClick={() => setStep(0)}>
                <ArrowLeft size={12} /> Back
              </button>
              <button className="btn ghost" onClick={() => load(false)} disabled={checking}>
                <RefreshCw size={12} className={checking ? "spin" : ""} /> Re-check
              </button>
              <div style={{ flex: 1 }} />
              <button className="btn primary" onClick={() => setStep(2)}>
                Continue <ArrowRight size={13} />
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="onb-eyebrow">
              <Wordmark /> · Step 3 of 3
            </div>
            <h2 className="onb-title">Usage analytics</h2>
            <p className="onb-sub">
              orx can send anonymous usage analytics to help improve the tool. No code, prompts,
              file contents, repo names, or project/session identifiers are ever sent — just a
              random per-install id, CLI version, OS and architecture, CI flag, coarse install
              type, and coarse events such as commands, onboarding completion, project creation,
              and chat-session starts.
            </p>
            <div className="onb-cards">
              <TelemetryCard
                telemetry={telemetry}
                saving={telemetrySaving}
                onSavingChange={setTelemetrySaving}
                onUpdate={setTelemetryState}
              />
            </div>
            {/* The data dir moved to Settings → Storage; still disclose where
                things land so the location isn't a surprise. */}
            <p className="onb-aside-text" style={{ marginTop: 12 }}>
              Your database, run logs, and artifacts stay on this machine — change where they live
              any time in Settings → Storage.
            </p>
            <div className="onb-actions">
              <button className="btn ghost" onClick={() => setStep(1)}>
                <ArrowLeft size={12} /> Back
              </button>
              <div style={{ flex: 1 }} />
              <button
                className="btn primary"
                onClick={finishOnboarding}
                disabled={telemetrySaving}
              >
                Create your first project <ArrowRight size={13} />
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** A shell command plus its own copy button, sharing one border so the button
 * reads as part of the command. Each chip owns its "Copied" state. */
function CmdChip({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard.writeText(cmd).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <span className="onb-cmd-chip">
      <code>{cmd}</code>
      <button
        className="onb-cmd-copy"
        onClick={copy}
        aria-label={copied ? "Copied" : "Copy command"}
        title={copied ? "Copied" : "Copy"}
      >
        {copied ? <Check size={13} strokeWidth={3} /> : <Copy size={13} />}
      </button>
    </span>
  );
}

/** Agent notes carry the command to run in backticks (`claude auth login`) —
 * render those spans as code so they read as something to type, not prose. */
function agentBadge(h: Harness): { cls: string; label: string } {
  if (h.agentReady) return { cls: "st-done", label: "Signed in" };
  if (!h.installed) return { cls: "st-idle", label: "Not detected" };
  if (h.authState === "unknown") return { cls: "st-starting", label: "Unable to verify" };
  if (h.authState === "unsupported") return { cls: "st-starting", label: "Update required" };
  if (h.installed) return { cls: "st-starting", label: "Not signed in" };
  return { cls: "st-idle", label: "Not detected" };
}

function AgentCard({ h }: { h: Harness }) {
  const badge = agentBadge(h);
  const version = h.version?.replace(/\s*\(.*\)$/, "");
  return (
    <div className="onb-card">
      <div className="onb-card-head">
        <span className="onb-card-name">{h.name}</span>
        <span className={`status-badge ${badge.cls}`}>
          {h.agentReady ? <Check size={12} strokeWidth={3} /> : <span className="dot" />}
          {badge.label}
        </span>
      </div>
      {h.agentReady ? (
        <>
          <div className="onb-card-detail mono">
            {h.account ?? "API key"}
            {h.plan ? ` · ${h.plan}` : ""}
          </div>
          <div className="onb-card-meta">
            {[
              version,
              h.models.length > 0 &&
                `${h.models.length} model${h.models.length === 1 ? "" : "s"} — ${h.models
                  .slice(0, 3)
                  .map((m) => harnessModelLabel(m))
                  .join(", ")}${h.models.length > 3 ? ", …" : ""}`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
        </>
      ) : (
        <div className="onb-card-meta">{renderNote(h.agentNote)}</div>
      )}
    </div>
  );
}

function GitCard({
  git,
  onUpdate,
}: {
  git: GitSettings | null;
  onUpdate: (g: GitSettings) => void;
}) {
  // The PAT form is the fallback, not a peer of `gh auth login` — keep it
  // behind a disclosure so the card offers one obvious action per row.
  const [tokenOpen, setTokenOpen] = useState(false);
  if (git === null) {
    return (
      <div className="onb-loading">
        <span className="spinner" /> Checking git…
      </div>
    );
  }
  if (!git.gitVersion) {
    return (
      <div className="onb-card">
        <div className="onb-card-head">
          <span className="onb-card-name">git</span>
          <span className="status-badge st-failed">
            <span className="dot" /> Not found
          </span>
        </div>
        <div className="onb-card-meta">Install git to clone projects, then re-open orx.</div>
      </div>
    );
  }
  const identity = [git.userName, git.userEmail && `<${git.userEmail}>`]
    .filter(Boolean)
    .join(" ");
  return (
    <div className="onb-card">
      <div className="onb-card-row">
        <span className="onb-card-name">GitHub</span>
        <span className="onb-card-detail mono">
          {git.githubTokenSource === "env"
            ? "Token from GITHUB_TOKEN"
            : git.githubTokenSource === "stored"
              ? "Token saved in orx"
              : git.githubTokenSource === "gh"
                ? "Signed in via gh CLI"
                : ""}
        </span>
        <span className={`status-badge ${git.githubTokenSource ? "st-done" : "st-starting"}`}>
          {git.githubTokenSource ? <Check size={12} strokeWidth={3} /> : <span className="dot" />}
          {git.githubTokenSource ? "Connected" : "Not connected"}
        </span>
      </div>
      {!git.githubTokenSource && (
        <>
          <div className="onb-fix">
            <span className="onb-fix-label">
              {tokenOpen
                ? "Paste a personal access token:"
                : git.ghInstalled
                  ? "Run this in your terminal to sign in:"
                  : "Install the GitHub CLI, then run this to sign in:"}
            </span>
            <button className="onb-fix-alt" onClick={() => setTokenOpen((v) => !v)}>
              {tokenOpen ? "Use gh instead" : "Paste a token instead"}
            </button>
          </div>
          {tokenOpen ? <GitTokenForm onSaved={onUpdate} /> : <CmdChip cmd="gh auth login" />}
        </>
      )}
      {!identity && (
        <div className="onb-aside">
          <div className="onb-aside-text">
            Optional — so the agent&apos;s commits are attributed to you:
          </div>
          <CmdChip cmd={`git config --global user.name "Your Name" && git config --global user.email "you@example.com"`} />
        </div>
      )}
    </div>
  );
}

function TelemetryCard({
  telemetry,
  saving,
  onSavingChange,
  onUpdate,
}: {
  telemetry: TelemetrySettings | null;
  saving: boolean;
  onSavingChange: (saving: boolean) => void;
  onUpdate: (t: TelemetrySettings) => void;
}) {
  if (telemetry === null) {
    return (
      <div className="onb-loading">
        <span className="spinner" /> Checking analytics…
      </div>
    );
  }
  const on = telemetry.enabled;
  // A per-run override (e.g. `--no-telemetry`) that isn't the persisted setting:
  // the toggle writes the persisted flag, but this run stays off regardless.
  const overridden = !on && telemetry.reason !== null && telemetry.reason !== "disabled via `orx telemetry off`";
  const choose = (enabled: boolean) => {
    if (saving || enabled === on) return;
    onSavingChange(true);
    void setTelemetry(enabled)
      .then(onUpdate)
      .catch(() => {})
      .finally(() => onSavingChange(false));
  };
  return (
    <div className="onb-card">
      <div className="onb-card-head">
        <div>
          <div className="onb-card-name">Share anonymous usage analytics</div>
          <div className="onb-card-meta" style={{ marginTop: 2 }}>
            {on
              ? "On — helps prioritize what to build next."
              : overridden
                ? `Off — ${telemetry.reason}.`
                : "Off — you can turn it back on anytime."}
          </div>
        </div>
        <div style={{ display: "flex", gap: 6, flex: "none" }}>
          <button
            className={`btn ${on ? "primary" : "ghost"}`}
            onClick={() => choose(true)}
            disabled={saving}
            aria-pressed={on}
          >
            {on ? <Check size={12} strokeWidth={3} /> : null} On
          </button>
          <button
            className={`btn ${!on ? "primary" : "ghost"}`}
            onClick={() => choose(false)}
            disabled={saving}
            aria-pressed={!on}
          >
            {!on ? <Check size={12} strokeWidth={3} /> : null} Off
          </button>
        </div>
      </div>
      <div className="onb-card-meta" style={{ marginTop: 12 }}>
        Sent: a random per-install id, CLI version, OS/architecture, CI flag, coarse install type,
        and coarse usage events. Never sent: code, prompts, file contents, paths, repo names, or
        project/session identifiers. Change anytime in Settings or with{" "}
        <code>orx telemetry off</code>.
      </div>
      {overridden && (
        <div className="onb-card-meta" style={{ marginTop: 8 }}>
          Note: this run is off because of {telemetry.reason}, which overrides the saved choice.
        </div>
      )}
    </div>
  );
}
