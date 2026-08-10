import { ArrowLeft, ArrowRight, Check, RefreshCw, X } from "lucide-react";
import { Wordmark } from "./Wordmark";
import { useEffect, useRef, useState } from "react";
import {
  getHarnesses,
  getProfile,
  getProjectPathStatus,
  harnessModelLabel,
  completeOnboarding,
  reasoningFor,
  searchPapers,
  type AgentSelection,
  type Harness,
  type HarnessId,
  type LinkedPaper,
  type PaperHit,
  type Project,
} from "../api";
import { renderNote } from "./agentNote";
import { onHarnessAuth } from "../events";
import { GHOST_BUTTON_CLASS_NAME, MONO_CLASS_NAME, PAPER_TITLE_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME, STATUS_BADGE_CLASS_NAME } from "../styleClasses";

const ONB_GATE_HINT_CLASS_NAME = [
  "onb-gate-hint [font-size:var(--fs-base)] [font-weight:var(--fw-semibold)] [line-height:1.5] [color:var(--text)]",
  "onb-agent-hint [margin:0_0_10px]",
].join(" ");

const ONB_CARD_META_CLASS_NAME = [
  "onb-card-meta [font-size:var(--fs-sm)] [color:var(--subtext)] [&_code]:[font-family:var(--mono)]",
  "[&_code]:[font-size:var(--fs-xs)] [&_code]:[background:var(--panel)]",
  "[&_code]:[border:1px_solid_var(--border-variant)] [&_code]:[border-radius:var(--radius-xs)]",
  "[&_code]:[padding:1px_5px] [&_code]:[white-space:nowrap]",
].join(" ");

const GIT_RETRY_HINT_CLASS_NAME = [
  "onb-gate-hint [margin:18px_0_0] [font-size:var(--fs-base)] [font-weight:var(--fw-semibold)] [line-height:1.5]",
  "[color:var(--text)] onb-git-hint [margin-top:8px]",
].join(" ");

const ONB_CARD_CLASS_NAME = [
  "onb-card [display:flex] [flex-direction:column] [gap:5px] [background:var(--base)]",
  "[border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [padding:18px_20px]",
].join(" ");

const FINISH_ERROR_CLASS_NAME = [
  "onb-gate-hint [margin:18px_0_0] [font-size:var(--fs-base)] [font-weight:var(--fw-semibold)] [line-height:1.5]",
  "[color:var(--text)]",
].join(" ");

const ONB_AGENT_LOGO_CLASS_NAME = [
  "onb-agent-logo [display:block] [width:18px] [height:18px] [flex:0_0_auto] [fill:currentColor]",
  "[&.claude]:[color:#d97757]",
].join(" ");

const RETRY_COPY = "Couldn't reach orx. Check it's still running, then re-check.";
const RESEARCH_AREAS = ["AI/ML", "Biology", "Physics", "Other"];

/** First-run walkthrough: choose a local coding agent, verify Git, add a
 * research profile, then install and open the demo project. The local tool
 * checks gate setup; the profile is saved best-effort so it never blocks
 * installation. The data-dir choice lives in
 * Settings → Storage (which can also *move* existing data); usage analytics is
 * opt-out via the CLI (`orx telemetry off`). */
export function Onboarding({
  onDone,
  preferredAgent,
}: {
  onDone: (project: Project, selection: AgentSelection) => void;
  preferredAgent: AgentSelection | null;
}) {
  const [step, setStep] = useState<0 | 1>(0);
  const [harnesses, setHarnesses] = useState<Harness[] | null>(null);
  const [gitVersion, setGitVersion] = useState<string | null>();
  const [finishing, setFinishing] = useState(false);
  const [finishError, setFinishError] = useState<string | null>(null);
  const [preferredHarness, setPreferredHarness] = useState<HarnessId | null>(null);
  const [checking, setChecking] = useState(false);
  const [researchAreas, setResearchAreas] = useState<string[]>([]);
  const [otherArea, setOtherArea] = useState("");
  const [background, setBackground] = useState("");
  const [papers, setPapers] = useState<LinkedPaper[]>([]);
  const [paperQuery, setPaperQuery] = useState("");
  const [paperHits, setPaperHits] = useState<PaperHit[]>([]);
  const [searchingPapers, setSearchingPapers] = useState(false);
  const paperSeq = useRef(0);
  // Per-probe, not one shared flag: a git failure must not put a connectivity
  // error on the harness gate it has nothing to do with — or worse, hide the
  // actionable "sign in" hint behind it.
  const [harnessError, setHarnessError] = useState(false);
  const [gitError, setGitError] = useState(false);

  // Step 1 requires one genuinely usable harness and local Git. Failed or
  // inconclusive detection never bypasses either gate.
  const anyAgentReady = harnesses?.some((h) => h.agentReady) ?? false;
  const gitReady = gitVersion != null;

  // Drops a slow probe whose answer a newer load has already superseded.
  const loadSeq = useRef(0);
  const load = (refresh: boolean, retryRejected = false) => {
    const seq = ++loadSeq.current;
    setChecking(true);
    setHarnessError(false);
    setGitError(false);
    setGitVersion(undefined);
    const fresh = () => seq === loadSeq.current;
    void Promise.allSettled([
      getHarnesses(refresh, retryRejected).then((h) => fresh() && setHarnesses(h)),
      getProjectPathStatus().then((status) => fresh() && setGitVersion(status.gitVersion)),
    ])
      .then(([harness, gitStatus]) => {
        if (!fresh()) return;
        // Clear the stale answer too, so "errored", "loading" and "loaded"
        // stay mutually exclusive — otherwise a failed re-check leaves old
        // cards on screen saying "not signed in" while the gate un-gates.
        if (harness.status === "rejected") {
          setHarnessError(true);
          setHarnesses(null);
        }
        if (gitStatus.status === "rejected") {
          setGitError(true);
          setGitVersion(undefined);
        }
      })
      .finally(() => fresh() && setChecking(false));
  };
  useEffect(() => load(false), []);
  useEffect(() => {
    if (harnesses === null) return;
    const ready = harnesses.filter((h) => h.agentReady);
    setPreferredHarness((current) => {
      if (current && ready.some((h) => h.id === current)) return current;
      const saved = preferredAgent && ready.find((h) => h.id === preferredAgent.harness);
      return saved?.id ?? ready[0]?.id ?? null;
    });
  }, [harnesses, preferredAgent]);
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
  // Prefill from any saved profile — best-effort, never gates the step.
  useEffect(() => {
    void getProfile()
      .then((p) => {
        setResearchAreas(p.researchAreas);
        setOtherArea(p.otherArea ?? "");
        setBackground(p.background ?? "");
        setPapers(p.papers);
      })
      .catch(() => {});
  }, []);

  // Debounced title search; `paperSeq` drops superseded responses.
  useEffect(() => {
    const q = paperQuery.trim();
    if (q.length < 3) {
      setPaperHits([]);
      setSearchingPapers(false);
      return;
    }
    const seq = ++paperSeq.current;
    setSearchingPapers(true);
    const t = setTimeout(() => {
      searchPapers(q)
        .then((res) => seq === paperSeq.current && setPaperHits(res))
        .catch(() => seq === paperSeq.current && setPaperHits([]))
        .finally(() => seq === paperSeq.current && setSearchingPapers(false));
    }, 350);
    return () => clearTimeout(t);
  }, [paperQuery]);

  const addPaper = (h: PaperHit) => {
    setPapers((cur) =>
      cur.some((p) => p.paperId === h.paperId)
        ? cur
        : [...cur, { paperId: h.paperId, title: cleanPaperTitle(h.title) }],
    );
    setPaperQuery("");
    setPaperHits([]);
  };
  const removePaper = (id: string) => setPapers((cur) => cur.filter((p) => p.paperId !== id));
  const toggleResearchArea = (area: string) => {
    setResearchAreas((current) =>
      current.includes(area) ? current.filter((item) => item !== area) : [...current, area],
    );
  };

  const researchProfileValid =
    researchAreas.length > 0 && (!researchAreas.includes("Other") || otherArea.trim().length > 0);

  const finishOnboarding = async () => {
    const harness = harnesses?.find((item) => item.id === preferredHarness && item.agentReady);
    if (!harness || finishing) return;
    const selection = selectionFor(harness);
    setFinishing(true);
    setFinishError(null);
    try {
      const completion = await completeOnboarding(selection, {
        researchAreas,
        otherArea: researchAreas.includes("Other") ? otherArea : null,
        background: background || null,
        papers,
      });
      onDone(completion.project, completion.selection);
    } catch (error) {
      setFinishError(error instanceof Error ? error.message : String(error));
    } finally {
      setFinishing(false);
    }
  };

  return (
    <div className="home [flex:1] [min-height:0] [overflow-y:auto] [scrollbar-gutter:stable_both-edges] [background:var(--canvas)] onboarding [&_.home-inner]:[max-width:560px] [&_.home-inner]:[padding-top:96px]">
      <div className="home-inner [max-width:620px] [margin:0_auto] [padding:48px_24px_64px]">
        {step === 0 ? (
          <>
            <div className="onb-eyebrow [font-size:var(--fs-xl)] [font-weight:var(--fw-semibold)] [color:var(--muted)] [margin-bottom:18px]">
              <Wordmark /> · Step 1 of 2
            </div>
            <h2 className="onb-title [margin:0_0_6px] [font-size:var(--fs-3xl)] [letter-spacing:-0.01em]">Choose a coding agent</h2>
            <p className="onb-sub [color:var(--text)] [font-size:var(--fs-base)] [line-height:1.55] [margin:0_0_22px] [max-width:480px]">OpenResearch uses an agent already signed in on this machine.</p>
            {harnesses !== null && !anyAgentReady && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                Sign in to at least one agent to continue.
              </p>
            )}
            {harnesses !== null && anyAgentReady && preferredHarness === null && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                Choose a coding agent to continue.
              </p>
            )}
            <div className="onb-cards [display:flex] [flex-direction:column] [gap:10px]">
              {harnesses !== null ? (
                harnesses.map((h) => (
                  <AgentCard
                    key={h.id}
                    h={h}
                    selected={preferredHarness === h.id}
                    onSelect={() => setPreferredHarness(h.id)}
                  />
                ))
              ) : harnessError ? (
                // Never a spinner next to an error — detection isn't running.
                <div className={ONB_CARD_META_CLASS_NAME}>{RETRY_COPY}</div>
              ) : (
                <div className="onb-loading [display:flex] [align-items:center] [gap:8px] [color:var(--subtext)] [font-size:var(--fs-md)] [padding:8px_0]">
                  <span className={SPINNER_CLASS_NAME} /> Detecting Claude Code, Codex, OpenCode…
                </div>
              )}
            </div>
            {(gitVersion === null || gitError) && (
              <div className="onb-git-check [margin-top:28px]" role="status" aria-live="polite">
                <LocalGitCard gitVersion={gitVersion} error={gitError} />
                {gitError ? (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>{RETRY_COPY}</p>
                ) : (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>
                    Git is required for local experiments. Install Git, then re-check.
                  </p>
                )}
              </div>
            )}
            <div className="onb-actions [display:flex] [align-items:center] [gap:10px] [margin-top:22px]">
              {(harnessError ||
                gitError ||
                gitVersion === null ||
                (harnesses !== null && !anyAgentReady)) && (
                <button className={GHOST_BUTTON_CLASS_NAME} onClick={() => load(true, true)} disabled={checking}>
                  <RefreshCw size={12} className={checking ? "spin [animation:settings-spin_0.9s_linear_infinite]" : ""} /> Re-check
                </button>
              )}
              <div style={{ flex: 1 }} />
              <button
                className={PRIMARY_BUTTON_CLASS_NAME}
                onClick={() => setStep(1)}
                disabled={checking || !anyAgentReady || preferredHarness === null || !gitReady}
                title={
                  checking
                    ? "Waiting for the local tool checks"
                    : !anyAgentReady
                      ? "Sign in to at least one coding agent to continue"
                      : preferredHarness === null
                        ? "Choose your preferred coding agent"
                        : gitError
                          ? "Re-check Git before continuing"
                          : gitVersion === undefined
                            ? "Waiting for the Git check"
                            : gitVersion === null
                              ? "Install Git to continue"
                              : undefined
                }
              >
                Continue <ArrowRight size={13} />
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="onb-eyebrow [font-size:var(--fs-xl)] [font-weight:var(--fw-semibold)] [color:var(--muted)] [margin-bottom:18px]">
              <Wordmark /> · Step 2 of 2
            </div>
            <h2 className="onb-title [margin:0_0_6px] [font-size:var(--fs-3xl)] [letter-spacing:-0.01em] onb-profile-title [margin-bottom:22px]">Tell us about your research</h2>
            <div className="onb-cards [display:flex] [flex-direction:column] [gap:10px]">
              <div className={ONB_CARD_CLASS_NAME}>
                <fieldset className="onb-fieldset [border:0] [margin:0_0_18px] [padding:0] [&_legend]:[font-size:var(--fs-base)] [&_legend]:[font-weight:var(--fw-semibold)] [&_legend]:[margin-bottom:6px]">
                  <legend>What areas are you interested in?</legend>
                  <p className="onb-field-hint [color:var(--muted)] [font-size:var(--fs-sm)] [line-height:1.4] [margin:0_0_8px]">Choose one or more.</p>
                  <div className="onb-area-options [display:grid] [grid-template-columns:repeat(2,_minmax(0,_1fr))] [gap:8px]">
                    {RESEARCH_AREAS.map((area) => (
                      <label key={area} className="onb-area-option [display:flex] [align-items:center] [gap:8px] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [cursor:pointer] [padding:9px_10px] [&:has(input:checked)]:[border-color:var(--accent)] [&:has(input:checked)]:[background:var(--primary-subtle)] [&_input]:[margin:0]">
                        <input
                          type="checkbox"
                          checked={researchAreas.includes(area)}
                          onChange={() => toggleResearchArea(area)}
                          disabled={finishing}
                        />
                        <span>{area}</span>
                      </label>
                    ))}
                  </div>
                  {researchAreas.includes("Other") && (
                    <input
                      className="onb-other-area [width:100%] [margin-top:8px]"
                      value={otherArea}
                      onChange={(event) => setOtherArea(event.target.value)}
                      disabled={finishing}
                      placeholder="Tell us your other research area"
                      aria-label="Other research area"
                    />
                  )}
                </fieldset>
                <label className="onb-field-label [font-size:var(--fs-base)] [font-weight:var(--fw-semibold)] [margin-bottom:6px]" htmlFor="onb-background">
                  Research background
                </label>
                <textarea
                  id="onb-background"
                  className="onb-textarea [width:100%] [resize:vertical] [min-height:78px] [line-height:1.5] [font-size:var(--fs-base)] [margin-bottom:14px]"
                  value={background}
                  onChange={(e) => setBackground(e.target.value)}
                  disabled={finishing}
                  rows={4}
                  placeholder="e.g. I work on sample-efficient RL for LLM post-training, focused on reward-model-free methods."
                />
                <label className="onb-field-label [font-size:var(--fs-base)] [font-weight:var(--fw-semibold)] [margin-bottom:6px]" htmlFor="onb-paper-search">
                  Representative papers
                </label>
                <p className="onb-field-hint [color:var(--muted)] [font-size:var(--fs-sm)] [line-height:1.4] [margin:0_0_8px]">
                  Add papers that represent your research interests, including papers by other
                  authors.
                </p>
                <div className="onb-paper-search [display:flex] [flex-direction:column] [gap:6px] [margin-top:12px] [&_input]:[width:100%]">
                  <input
                    id="onb-paper-search"
                    value={paperQuery}
                    onChange={(e) => setPaperQuery(e.target.value)}
                    disabled={finishing}
                    placeholder="Search alphaXiv by title to link a paper…"
                  />
                  {searchingPapers ? (
                    <div className={ONB_CARD_META_CLASS_NAME}>Searching alphaXiv…</div>
                  ) : paperHits.length > 0 ? (
                    <div className="onb-paper-results [display:flex] [flex-direction:column] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [max-height:200px] [overflow-y:auto] [&_button]:[display:flex] [&_button]:[flex-direction:column] [&_button]:[align-items:flex-start] [&_button]:[gap:2px] [&_button]:[padding:8px_10px] [&_button]:[background:none] [&_button]:[border:none] [&_button]:[border-bottom:1px_solid_var(--border-variant)] [&_button]:[text-align:left] [&_button]:[font:inherit] [&_button]:[color:var(--text)] [&_button]:[cursor:pointer] [&_button:last-child]:[border-bottom:none] [&_button:hover]:[background:var(--surface)] [&_.title]:[font-size:var(--fs-md)] [&_.title]:[font-weight:var(--fw-medium)] [&_.id]:[font-family:var(--mono)] [&_.id]:[font-size:var(--fs-xs)] [&_.id]:[color:var(--muted)]">
                      {paperHits.map((h) => (
                        <button
                          key={h.paperId}
                          type="button"
                          onClick={() => addPaper(h)}
                          disabled={finishing}
                        >
                          <span className={PAPER_TITLE_CLASS_NAME}>{cleanPaperTitle(h.title)}</span>
                          <span className="id">{h.paperId}</span>
                        </button>
                      ))}
                    </div>
                  ) : null}
                </div>
                {papers.length > 0 && (
                  <div className="onb-paper-chips [display:flex] [flex-wrap:wrap] [gap:6px] [margin-top:10px]">
                    {papers.map((p) => (
                      <span key={p.paperId} className="onb-paper-chip [display:inline-flex] [align-items:center] [gap:6px] [padding:4px_4px_4px_10px] [border:1px_solid_var(--border)] [border-radius:var(--radius-sm)] [background:var(--surface)] [font-size:var(--fs-sm)] [max-width:100%] [&_.title]:[font-weight:var(--fw-medium)] [&_.title]:[overflow:hidden] [&_.title]:[text-overflow:ellipsis] [&_.title]:[white-space:nowrap] [&_.title]:[max-width:240px] [&_.id]:[font-family:var(--mono)] [&_.id]:[font-size:var(--fs-xs)] [&_.id]:[color:var(--muted)] [&_button]:[display:inline-flex] [&_button]:[align-items:center] [&_button]:[justify-content:center] [&_button]:[padding:2px] [&_button]:[border:none] [&_button]:[background:none] [&_button]:[color:var(--muted)] [&_button]:[cursor:pointer] [&_button]:[border-radius:var(--radius-xs)] [&_button:hover]:[color:var(--text)] [&_button:hover]:[background:var(--panel)]">
                        <span className={PAPER_TITLE_CLASS_NAME}>{p.title || p.paperId}</span>
                        <span className="id">{p.paperId}</span>
                        <button
                          type="button"
                          aria-label={`Remove ${p.paperId}`}
                          onClick={() => removePaper(p.paperId)}
                          disabled={finishing}
                        >
                          <X size={12} />
                        </button>
                      </span>
                    ))}
                  </div>
                )}
              </div>
            </div>
            {!researchProfileValid && (
              <p className="onb-profile-hint [color:var(--accent-red)] [font-size:var(--fs-sm)] [margin:8px_0_0]">
                {researchAreas.length === 0
                  ? "Choose at least one research area to continue."
                  : "Describe your research area to continue."}
              </p>
            )}
            <div className="onb-actions [display:flex] [align-items:center] [gap:10px] [margin-top:22px]">
              <button className={GHOST_BUTTON_CLASS_NAME} onClick={() => setStep(0)} disabled={finishing}>
                <ArrowLeft size={12} /> Back
              </button>
              <div style={{ flex: 1 }} />
              <button
                className={PRIMARY_BUTTON_CLASS_NAME}
                onClick={() => void finishOnboarding()}
                disabled={finishing || preferredHarness === null || !researchProfileValid}
              >
                {finishing ? (
                  <>
                    <span className={SPINNER_CLASS_NAME} /> Setting things up…
                  </>
                ) : (
                  <>
                    Get started <ArrowRight size={13} />
                  </>
                )}
              </button>
            </div>
            {preferredHarness === null && (
              <p className={FINISH_ERROR_CLASS_NAME}>
                Your selected agent is no longer ready. Go back to Step 1 and choose another.
              </p>
            )}
            {finishError && <p className={FINISH_ERROR_CLASS_NAME}>{finishError}</p>}
          </>
        )}
      </div>
    </div>
  );
}

/** Fast-search titles carry scrape cruft: "[1706.03762] Title - arXiv".
 * Kept in sync with NewProjectForm's cleanTitle. */
function cleanPaperTitle(title: string): string {
  return title.replace(/^\[[^\]]*\]\s*/, "").replace(/\s*[-–|]\s*arXiv\s*$/i, "");
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

function selectionFor(harness: Harness): AgentSelection {
  const model = harness.models[0]?.id ?? null;
  return {
    harness: harness.id,
    model,
    permissionMode: harness.options?.defaultPermissionMode ?? null,
    reasoningLevel: reasoningFor(harness, model).defaultId,
  };
}

function AgentLogo({ harness }: { harness: HarnessId }) {
  if (harness === "claude-code") {
    return (
      <svg className="onb-agent-logo [display:block] [width:18px] [height:18px] [flex:0_0_auto] [fill:currentColor] [&.claude]:[color:#d97757] claude" viewBox="0 0 24 24" aria-hidden="true">
        <path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" />
      </svg>
    );
  }
  if (harness === "opencode") {
    return (
      <svg className={ONB_AGENT_LOGO_CLASS_NAME} viewBox="0 0 24 24" aria-hidden="true">
        <path d="M22 24H2V0h20zM17 4.8H7v14.4h10z" />
      </svg>
    );
  }
  return (
    <svg className={ONB_AGENT_LOGO_CLASS_NAME} viewBox="146 227 268 265" aria-hidden="true">
      <path d="M249.176 323.434V298.276C249.176 296.158 249.971 294.569 251.825 293.509L302.406 264.381C309.29 260.409 317.5 258.555 325.973 258.555C357.75 258.555 377.877 283.185 377.877 309.399C377.877 311.253 377.877 313.371 377.611 315.49L325.178 284.771C322.001 282.919 318.822 282.919 315.645 284.771L249.176 323.434ZM367.283 421.415V361.301C367.283 357.592 365.694 354.945 362.516 353.092L296.048 314.43L317.763 301.982C319.617 300.925 321.206 300.925 323.058 301.982L373.639 331.112C388.205 339.586 398.003 357.592 398.003 375.069C398.003 395.195 386.087 413.733 367.283 421.412V421.415ZM233.553 368.452L211.838 355.742C209.986 354.684 209.19 353.095 209.19 350.975V292.718C209.19 264.383 230.905 242.932 260.301 242.932C271.423 242.932 281.748 246.641 290.49 253.26L238.321 283.449C235.146 285.303 233.555 287.951 233.555 291.659V368.455L233.553 368.452ZM280.292 395.462L249.176 377.985V340.913L280.292 323.436L311.407 340.913V377.985L280.292 395.462ZM300.286 475.968C289.163 475.968 278.837 472.259 270.097 465.64L322.264 435.449C325.441 433.597 327.03 430.949 327.03 427.239V350.445L349.011 363.155C350.865 364.213 351.66 365.802 351.66 367.922V426.179C351.66 454.514 329.679 475.965 300.286 475.965V475.968ZM237.525 416.915L186.944 387.785C172.378 379.31 162.582 361.305 162.582 343.827C162.582 323.436 174.763 305.164 193.563 297.485V357.861C193.563 361.571 195.154 364.217 198.33 366.071L264.535 404.467L242.82 416.915C240.967 417.972 239.377 417.972 237.525 416.915ZM234.614 460.343C204.689 460.343 182.71 437.833 182.71 410.028C182.71 407.91 182.976 405.792 183.238 403.672L235.405 433.863C238.582 435.715 241.763 435.715 244.938 433.863L311.407 395.466V420.622C311.407 422.742 310.612 424.331 308.758 425.389L258.179 454.519C251.293 458.491 243.083 460.343 234.611 460.343H234.614ZM300.286 491.854C332.329 491.854 359.073 469.082 365.167 438.892C394.825 431.211 413.892 403.406 413.892 375.073C413.892 356.535 405.948 338.529 391.648 325.552C392.972 319.991 393.766 314.43 393.766 308.87C393.766 271.003 363.048 242.666 327.562 242.666C320.413 242.666 313.528 243.723 306.644 246.109C294.725 234.457 278.307 227.042 260.301 227.042C228.258 227.042 201.513 249.815 195.42 280.004C165.761 287.685 146.694 315.49 146.694 343.824C146.694 362.362 154.638 380.368 168.938 393.344C167.613 398.906 166.819 404.467 166.819 410.027C166.819 447.894 197.538 476.231 233.024 476.231C240.172 476.231 247.058 475.173 253.943 472.788C265.859 484.441 282.278 491.854 300.286 491.854Z" />
    </svg>
  );
}

function AgentCard({
  h,
  selected,
  onSelect,
}: {
  h: Harness;
  selected: boolean;
  onSelect: () => void;
}) {
  const badge = agentBadge(h);
  const visibleBadge = selected ? { cls: "st-done", label: "Selected" } : badge;
  const version = h.version?.replace(/\s*\(.*\)$/, "");
  const head = (
    <div className="onb-card-head [display:flex] [align-items:center] [justify-content:space-between] [gap:12px]">
      <span className="onb-card-identity [display:flex] [align-items:center] [gap:10px] [min-width:0]">
        <AgentLogo harness={h.id} />
        <span className="onb-card-name [font-weight:var(--fw-semibold)] [font-size:var(--fs-base)]">{h.name}</span>
      </span>
      <span className={`${STATUS_BADGE_CLASS_NAME} ${visibleBadge.cls}`}>
        {h.agentReady ? <Check size={12} strokeWidth={3} /> : <span className="dot" />}
        {visibleBadge.label}
      </span>
    </div>
  );
  // An unready agent can't be selected — render it as a plain container, not a
  // disabled button, so the copy button on its `agentNote` command stays live.
  if (!h.agentReady) {
    return (
      <div className="onb-card [display:flex] [flex-direction:column] [gap:5px] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [padding:18px_20px] onb-agent-choice [width:100%] [color:inherit] [font:inherit] [text-align:left] [transition:border-color_120ms_ease,_box-shadow_120ms_ease] [button&]:[cursor:pointer] [button&:hover]:[border-color:var(--muted)] [&.selected]:[border-color:var(--accent)] [&.selected]:[box-shadow:0_0_0_1px_var(--accent)]">
        {head}
        <div className={ONB_CARD_META_CLASS_NAME}>{renderNote(h.agentNote)}</div>
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`onb-card [display:flex] [flex-direction:column] [gap:5px] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [padding:18px_20px] onb-agent-choice [width:100%] [color:inherit] [font:inherit] [text-align:left] [transition:border-color_120ms_ease,_box-shadow_120ms_ease] [button&]:[cursor:pointer] [button&:hover]:[border-color:var(--muted)] [&.selected]:[border-color:var(--accent)] [&.selected]:[box-shadow:0_0_0_1px_var(--accent)]${selected ? " selected" : ""}`}
      aria-pressed={selected}
      onClick={onSelect}
    >
      {head}
      <div className={`onb-card-detail ${MONO_CLASS_NAME}`}>
        {h.account ?? "API key"}
        {h.plan ? ` · ${h.plan}` : ""}
      </div>
      <div className={ONB_CARD_META_CLASS_NAME}>
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
    </button>
  );
}

function LocalGitCard({
  gitVersion,
  error,
}: {
  gitVersion: string | null | undefined;
  error: boolean;
}) {
  return (
    <div className={ONB_CARD_CLASS_NAME}>
      <div className="onb-card-head [display:flex] [align-items:center] [justify-content:space-between] [gap:12px]">
        <span className="onb-card-name [font-weight:var(--fw-semibold)] [font-size:var(--fs-base)]">Local Git</span>
        <span
          className={`${STATUS_BADGE_CLASS_NAME} ${gitVersion ? "st-done" : error || gitVersion === null ? "st-failed" : "st-starting"}`}
        >
          {gitVersion ? <Check size={12} strokeWidth={3} /> : <span className="dot" />}
          {gitVersion ? "Ready" : error ? "Check failed" : gitVersion === null ? "Not found" : "Checking"}
        </span>
      </div>
      {(gitVersion || (!error && gitVersion === undefined)) && (
        <div className={ONB_CARD_META_CLASS_NAME}>{gitVersion ?? "Checking Git…"}</div>
      )}
    </div>
  );
}
