import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
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
  resolvePaper,
  searchPapers,
  type AgentSelection,
  type Harness,
  type HarnessId,
  type LinkedPaper,
  type PaperHit,
  type Project,
} from "../api";
import { renderNote } from "./agentNote";
import { HarnessLogo } from "./HarnessLogo";
import { onHarnessAuth } from "../events";
import { GHOST_BUTTON_CLASS_NAME, MONO_CLASS_NAME, PAPER_TITLE_CLASS_NAME, PRIMARY_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME, STATUS_BADGE_CLASS_NAME } from "../styleClasses";

const ONB_GATE_HINT_CLASS_NAME = [
  "onb-gate-hint text-base font-semibold leading-normal text-text",
  "onb-agent-hint mt-0 mx-0 mb-2.5",
].join(" ");

const ONB_CARD_META_CLASS_NAME = [
  "onb-card-meta text-sm text-subtext [&_code]:font-mono",
  "[&_code]:text-xs [&_code]:bg-panel",
  "[&_code]:border [&_code]:border-border-variant [&_code]:rounded-xs",
  "[&_code]:py-px [&_code]:px-[5px] [&_code]:whitespace-nowrap",
].join(" ");

const GIT_RETRY_HINT_CLASS_NAME = [
  "onb-gate-hint mt-4.5 mx-0 mb-0 text-base font-semibold leading-normal",
  "text-text onb-git-hint mt-2",
].join(" ");

const ONB_CARD_CLASS_NAME = [
  "onb-card flex flex-col gap-[5px] bg-background",
  "border border-border rounded-lg py-4.5 px-5",
].join(" ");

const FINISH_ERROR_CLASS_NAME = [
  "onb-gate-hint mt-4.5 mx-0 mb-0 text-base font-semibold leading-normal",
  "text-text",
].join(" ");

const RESEARCH_AREAS = [
  { id: "AI/ML", label: m.onboarding_area_ai_ml },
  { id: "Biology", label: m.onboarding_area_biology },
  { id: "Physics", label: m.onboarding_area_physics },
  { id: "Other", label: m.onboarding_area_other },
];

/** First-run walkthrough: choose a local coding agent, verify Git, add a
 * research profile, then install and open the demo project. The local tool
 * checks gate setup; the profile is saved best-effort so it never blocks
 * installation. The data-dir choice lives in
 * Settings → Storage (which can also *move* existing data); usage analytics is
 * opt-out via Settings or `orx telemetry off`. */
export function Onboarding({
  onDone,
  preferredAgent,
}: {
  onDone: (project: Project, selection: AgentSelection) => void;
  preferredAgent: AgentSelection | null;
}) {
  const [step, setStep] = useState<0 | 1 | 2>(0);
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
    const duplicate = papers.some((p) => p.paperId === h.paperId);
    setPapers((cur) =>
      cur.some((p) => p.paperId === h.paperId)
        ? cur
        : [...cur, { paperId: h.paperId, title: cleanPaperTitle(h.title) }],
    );
    setPaperQuery("");
    setPaperHits([]);
    // The search hit's title is a Google-scraped string — truncated, id-prefixed,
    // sometimes reworded. Resolve the canonical title and correct it in place.
    if (!duplicate) {
      void resolvePaper(h.paperId)
        .then((r) => {
          const title = r.title?.trim();
          if (!title) return;
          setPapers((cur) => cur.map((p) => (p.paperId === h.paperId ? { ...p, title } : p)));
        })
        .catch(() => {});
    }
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
    <div
      className={`home flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges] bg-canvas onboarding ${
        step === 0
          ? "[&_.home-inner]:max-w-300 [&_.home-inner]:pt-0 [&_.home-inner]:pb-0"
          : "[&_.home-inner]:max-w-140 [&_.home-inner]:pt-24"
      }`}
    >
      <div
        className={`home-inner max-w-155 my-0 mx-auto ${
          step === 0 ? "px-8 sm:px-12" : "pt-12 px-6 pb-16"
        }`}
      >
        {step === 0 ? (
          <div className="onb-intro relative flex min-h-dvh flex-col justify-center gap-4 py-12 min-[1120px]:grid min-[1120px]:grid-cols-[minmax(0,_1.1fr)_minmax(28rem,_1fr)] min-[1120px]:grid-rows-[auto_auto] min-[1120px]:content-center min-[1120px]:gap-x-20 min-[1120px]:gap-y-10">
            <div className="onb-intro-copy relative z-10 min-[1120px]:col-start-1 min-[1120px]:row-start-1 min-[1120px]:self-start">
              <div className="onb-intro-brand text-[4rem] leading-none font-semibold tracking-[-0.035em] mb-10">
                <Wordmark />
              </div>
              <h2 className="onb-title mt-0 mx-0 text-[2.5rem] leading-[1.08] tracking-[-0.035em]">
                {m.onboarding_a_workspace_for_your_research_agents()}
              </h2>
            </div>
            <div className="onb-intro-features relative min-[1120px]:col-start-2 min-[1120px]:row-start-1 min-[1120px]:self-end">
              <div
                aria-hidden="true"
                className="absolute -inset-14 rounded-full bg-primary-subtle opacity-70 blur-3xl"
              />
              <ul className="onb-intro-list relative flex flex-col gap-4 m-0 p-0 list-none">
                <li className="rounded-2xl border border-border bg-background p-6 shadow-[0_14px_36px_color-mix(in_oklab,_var(--text)_6%,_transparent)]">
                  <span>
                    <strong className="mb-1.5 block text-2xl tracking-[-0.015em]">
                      {m.onboarding_consolidate_your_research()}
                    </strong>
                    <span className="block text-lg leading-[1.55] text-text">
                      {m.onboarding_track_experiments_artifacts_compute_skills_and_code_all()}
                    </span>
                  </span>
                </li>
                <li className="rounded-2xl border border-border bg-background p-6 shadow-[0_14px_36px_color-mix(in_oklab,_var(--text)_6%,_transparent)]">
                  <span>
                    <strong className="mb-1.5 block text-2xl tracking-[-0.015em]">
                      {m.onboarding_ground_your_agents()}
                    </strong>
                    <span className="block text-lg leading-[1.55] text-text">
                      {m.onboarding_sources_description()}
                    </span>
                  </span>
                </li>
                <li className="rounded-2xl border border-border bg-background p-6 shadow-[0_14px_36px_color-mix(in_oklab,_var(--text)_6%,_transparent)]">
                  <span>
                    <strong className="mb-1.5 block text-2xl tracking-[-0.015em]">
                      {m.onboarding_everything_stays_local()}
                    </strong>
                    <span className="block text-lg leading-[1.55] text-text">
                      {m.onboarding_your_code_data_and_experiment_history_stay_on()}
                    </span>
                  </span>
                </li>
              </ul>
            </div>
            <div className="onb-intro-actions relative z-10 mt-8 flex justify-end min-[1120px]:col-start-2 min-[1120px]:row-start-2 min-[1120px]:mt-0 min-[1120px]:self-start">
              <button
                className={`${PRIMARY_BUTTON_CLASS_NAME} !py-3.5 !px-7 !text-xl !rounded-lg`}
                onClick={() => setStep(1)}
              >
                {m.onboarding_continue()} <ArrowRight size={20} />
              </button>
            </div>
          </div>
        ) : step === 1 ? (
          <>
            <div className="onb-eyebrow text-xl font-semibold text-muted mb-4.5">
              <Wordmark /> {m.onboarding_step_1_of_2()}
            </div>
            <h2 className="onb-title mt-0 mx-0 mb-1.5 text-3xl tracking-[-0.01em]">{m.onboarding_choose_a_coding_agent()}</h2>
            <p className="onb-sub text-text text-base leading-[1.55] mt-0 mx-0 mb-5.5 max-w-120">{m.onboarding_open_research_uses_a_coding_agent_already_installed()}</p>
            {harnesses !== null && !anyAgentReady && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                {m.onboarding_sign_in_to_at_least_one_agent_to()}
              </p>
            )}
            {harnesses !== null && anyAgentReady && preferredHarness === null && (
              <p className={ONB_GATE_HINT_CLASS_NAME}>
                {m.onboarding_choose_a_coding_agent_to_continue()}
              </p>
            )}
            <div className="onb-cards flex flex-col gap-3.5">
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
                <div className={ONB_CARD_META_CLASS_NAME}>{m.onboarding_retry_connection()}</div>
              ) : (
                <div className="onb-loading flex items-center gap-2 text-subtext text-md py-2 px-0">
                  <span className={SPINNER_CLASS_NAME} /> {m.onboarding_detecting_claude_code_codex_open_code()}
                </div>
              )}
            </div>
            {(gitVersion === null || gitError) && (
              <div className="onb-git-check mt-7" role="status" aria-live="polite">
                <LocalGitCard gitVersion={gitVersion} error={gitError} />
                {gitError ? (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>{m.onboarding_retry_connection()}</p>
                ) : (
                  <p className={GIT_RETRY_HINT_CLASS_NAME}>
                    {m.onboarding_git_is_required_for_local_experiments_install_git()}
                  </p>
                )}
              </div>
            )}
            <div className="onb-actions flex items-center gap-2.5 mt-5.5">
              <button className={GHOST_BUTTON_CLASS_NAME} onClick={() => setStep(0)}>
                <ArrowLeft size={12} /> {m.onboarding_back()}
              </button>
              {(harnessError ||
                gitError ||
                gitVersion === null ||
                (harnesses !== null && !anyAgentReady)) && (
                <button className={GHOST_BUTTON_CLASS_NAME} onClick={() => load(true, true)} disabled={checking}>
                  <RefreshCw size={12} className={checking ? "spin animate-[settings-spin_0.9s_linear_infinite]" : ""} /> {m.onboarding_re_check()}
                </button>
              )}
              <div style={{ flex: 1 }} />
              <button
                className={PRIMARY_BUTTON_CLASS_NAME}
                onClick={() => setStep(2)}
                disabled={checking || !anyAgentReady || preferredHarness === null || !gitReady}
                title={
                  checking
                    ? m.onboarding_waiting_tool_checks()
                    : !anyAgentReady
                      ? m.onboarding_sign_in_agent_to_continue()
                      : preferredHarness === null
                        ? m.onboarding_choose_preferred_agent()
                        : gitError
                          ? m.onboarding_recheck_git_to_continue()
                          : gitVersion === undefined
                            ? m.onboarding_waiting_git_check()
                            : gitVersion === null
                              ? m.onboarding_install_git_to_continue()
                              : undefined
                }
              >
                {m.onboarding_continue()} <ArrowRight size={13} />
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="onb-eyebrow text-xl font-semibold text-muted mb-4.5">
              <Wordmark /> {m.onboarding_step_2_of_2()}
            </div>
            <h2 className="onb-title mt-0 mx-0 mb-1.5 text-3xl tracking-[-0.01em] onb-profile-title mb-5.5">{m.onboarding_tell_us_about_your_research()}</h2>
            <div className="onb-cards flex flex-col gap-2.5">
              <div className={ONB_CARD_CLASS_NAME}>
                <fieldset className="onb-fieldset border-0 mt-0 mx-0 mb-4.5 p-0 [&_legend]:text-base [&_legend]:font-semibold [&_legend]:mb-1.5">
                  <legend>{m.onboarding_what_areas_are_you_interested_in()}</legend>
                  <p className="onb-field-hint text-muted text-sm leading-[1.4] mt-0 mx-0 mb-2">{m.onboarding_choose_one_or_more()}</p>
                  <div className="onb-area-options grid grid-cols-[repeat(2,_minmax(0,_1fr))] gap-2">
                    {RESEARCH_AREAS.map((area) => (
                      <label key={area.id} className="onb-area-option flex items-center gap-2 border border-border rounded-md cursor-pointer py-[9px] px-2.5 [&:has(input:checked)]:border-accent [&:has(input:checked)]:bg-primary-subtle [&_input]:m-0">
                        <input
                          type="checkbox"
                          checked={researchAreas.includes(area.id)}
                          onChange={() => toggleResearchArea(area.id)}
                          disabled={finishing}
                        />
                        <span>{area.label()}</span>
                      </label>
                    ))}
                  </div>
                  {researchAreas.includes("Other") && (
                    <input
                      className="onb-other-area w-full mt-2"
                      value={otherArea}
                      onChange={(event) => setOtherArea(event.target.value)}
                      disabled={finishing}
                      placeholder={m.onboarding_tell_us_your_other_research_area()}
                      aria-label={m.onboarding_other_research_area()}
                    />
                  )}
                </fieldset>
                <label className="onb-field-label text-base font-semibold mb-1.5" htmlFor="onb-background">
                  {m.onboarding_research_background()}
                </label>
                <textarea
                  id="onb-background"
                  className="onb-textarea w-full resize-y min-h-19.5 leading-normal text-base mb-3.5"
                  value={background}
                  onChange={(e) => setBackground(e.target.value)}
                  disabled={finishing}
                  rows={4}
                  placeholder={m.onboarding_e_g_i_work_on_sample_efficient_rl()}
                />
                <label className="onb-field-label text-base font-semibold mb-1.5" htmlFor="onb-paper-search">
                  {m.onboarding_representative_papers()}
                </label>
                <p className="onb-field-hint text-muted text-sm leading-[1.4] mt-0 mx-0 mb-2">
                  {m.onboarding_add_papers_that_represent_your_research_interests_including()}
                </p>
                <div className="onb-paper-search flex flex-col gap-1.5 mt-3 [&_input]:w-full">
                  <input
                    id="onb-paper-search"
                    value={paperQuery}
                    onChange={(e) => setPaperQuery(e.target.value)}
                    disabled={finishing}
                    placeholder={m.onboarding_search_alpha_xiv_by_title_to_link_a()}
                  />
                  {searchingPapers ? (
                    <div className={ONB_CARD_META_CLASS_NAME}>{m.onboarding_searching_alpha_xiv()}</div>
                  ) : paperHits.length > 0 ? (
                    <div className="onb-paper-results flex flex-col border border-border rounded-md max-h-50 overflow-y-auto [&_button]:flex [&_button]:flex-col [&_button]:items-start [&_button]:gap-0.5 [&_button]:py-2 [&_button]:px-2.5 [&_button]:bg-none [&_button]:bg-transparent [&_button]:border-0 [&_button]:border-b [&_button]:border-b-border-variant [&_button]:text-start [&_button]:[font:inherit] [&_button]:text-text [&_button]:cursor-pointer [&_button:last-child]:border-b-0 [&_button:hover]:bg-surface [&_.title]:text-md [&_.title]:font-medium [&_.id]:font-mono [&_.id]:text-xs [&_.id]:text-muted">
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
                  <div className="onb-paper-chips flex flex-wrap gap-1.5 mt-2.5">
                    {papers.map((p) => (
                      <span key={p.paperId} className="onb-paper-chip inline-flex items-center gap-1.5 pt-1 pe-1 pb-1 ps-2.5 border border-border rounded-sm bg-surface text-sm max-w-full [&_.title]:font-medium [&_.title]:overflow-hidden [&_.title]:text-ellipsis [&_.title]:whitespace-nowrap [&_.title]:max-w-60 [&_.id]:font-mono [&_.id]:text-xs [&_.id]:text-muted [&_button]:inline-flex [&_button]:items-center [&_button]:justify-center [&_button]:p-0.5 [&_button]:border-0 [&_button]:bg-none [&_button]:bg-transparent [&_button]:text-muted [&_button]:cursor-pointer [&_button]:rounded-xs [&_button:hover]:text-text [&_button:hover]:bg-panel">
                        <span className={PAPER_TITLE_CLASS_NAME}>{p.title || p.paperId}</span>
                        <span className="id">{p.paperId}</span>
                        <button
                          type="button"
                          aria-label={m.a11y_remove_item({ name: ltr(p.paperId) })}
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
              <p className="onb-profile-hint text-accent-red text-sm mt-2 mx-0 mb-0">
                {researchAreas.length === 0
                  ? m.onboarding_choose_area_to_continue()
                  : m.onboarding_describe_area_to_continue()}
              </p>
            )}
            <div className="onb-actions flex items-center gap-2.5 mt-5.5">
              <button className={GHOST_BUTTON_CLASS_NAME} onClick={() => setStep(1)} disabled={finishing}>
                <ArrowLeft size={12} /> {m.onboarding_back()}
              </button>
              <div style={{ flex: 1 }} />
              <button
                className={PRIMARY_BUTTON_CLASS_NAME}
                onClick={() => void finishOnboarding()}
                disabled={finishing || preferredHarness === null || !researchProfileValid}
              >
                {finishing ? (
                  <>
                    <span className={SPINNER_CLASS_NAME} /> {m.onboarding_setting_things_up()}
                  </>
                ) : (
                  <>
                    {m.onboarding_get_started()} <ArrowRight size={13} />
                  </>
                )}
              </button>
            </div>
            {preferredHarness === null && (
              <p className={FINISH_ERROR_CLASS_NAME}>
                {m.onboarding_your_selected_agent_is_no_longer_ready_go()}
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
  if (h.agentReady) return { cls: "st-done", label: m.onboarding_signed_in() };
  if (!h.installed) return { cls: "st-idle", label: m.onboarding_not_detected() };
  if (h.installBroken) return { cls: "st-starting", label: m.onboarding_install_broken() };
  if (h.authState === "unknown") return { cls: "st-starting", label: m.onboarding_unable_to_verify() };
  if (h.authState === "unsupported") return { cls: "st-starting", label: m.onboarding_update_required() };
  if (h.installed) return { cls: "st-starting", label: m.onboarding_not_signed_in() };
  return { cls: "st-idle", label: m.onboarding_not_detected() };
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
  return <HarnessLogo harness={harness} size={26} />;
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
  const visibleBadge = selected ? { cls: "st-done", label: m.onboarding_selected() } : badge;
  const version = h.version?.replace(/\s*\(.*\)$/, "");
  const meta = [
    version,
    h.models.length > 0 &&
      `${h.models.length} model${h.models.length === 1 ? "" : "s"} — ${h.models
        .slice(0, 3)
        .map((m) => harnessModelLabel(m))
        .join(", ")}${h.models.length > 3 ? ", …" : ""}`,
  ]
    .filter(Boolean)
    .join(" · ");
  const head = (
    <div className="onb-card-head flex items-center justify-between gap-3">
      <span className="onb-card-identity flex items-center gap-3 min-w-0">
        <AgentLogo harness={h.id} />
        <span className="onb-card-name text-xl font-semibold tracking-[-0.01em]">{h.name}</span>
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
      <div className="onb-card flex flex-col gap-2.5 bg-background border border-border rounded-lg py-5.5 px-6 onb-agent-choice w-full text-inherit [font:inherit] text-start transition-[border-color,box-shadow] duration-120 ease-standard [button&]:cursor-pointer [button&:hover]:border-muted [&.selected]:border-accent [&.selected]:shadow-[0_0_0_1px_var(--accent)]">
        {head}
        <div className={ONB_CARD_META_CLASS_NAME}>{renderNote(h.agentNote)}</div>
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`onb-card flex flex-col gap-2.5 bg-background border border-border rounded-lg py-5.5 px-6 onb-agent-choice w-full text-inherit [font:inherit] text-start transition-[border-color,box-shadow] duration-120 ease-standard [button&]:cursor-pointer [button&:hover]:border-muted [&.selected]:border-accent [&.selected]:shadow-[0_0_0_1px_var(--accent)]${selected ? " selected" : ""}`}
      aria-pressed={selected}
      onClick={onSelect}
    >
      {head}
      <div className={`onb-card-detail ${MONO_CLASS_NAME}`}>
        {h.account ?? m.onboarding_api_key()}
        {h.plan ? ` · ${h.plan}` : ""}
      </div>
      <div
        className={`${ONB_CARD_META_CLASS_NAME} w-full overflow-hidden text-ellipsis whitespace-nowrap`}
        title={meta}
      >
        {meta}
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
      <div className="onb-card-head flex items-center justify-between gap-3">
        <span className="onb-card-name font-semibold text-base">{m.onboarding_local_git()}</span>
        <span
          className={`${STATUS_BADGE_CLASS_NAME} ${gitVersion ? "st-done" : error || gitVersion === null ? "st-failed" : "st-starting"}`}
        >
          {gitVersion ? <Check size={12} strokeWidth={3} /> : <span className="dot" />}
          {gitVersion ? m.onboarding_ready() : error ? m.onboarding_check_failed() : gitVersion === null ? m.onboarding_not_found() : m.onboarding_checking()}
        </span>
      </div>
      {(gitVersion || (!error && gitVersion === undefined)) && (
        <div className={ONB_CARD_META_CLASS_NAME}>{gitVersion ?? m.onboarding_checking_git()}</div>
      )}
    </div>
  );
}
