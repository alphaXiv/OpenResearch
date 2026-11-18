import { useContext, useEffect, useId, useState } from "react";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { useQuery } from "@tanstack/react-query";
import { Check, KeyRound, ShieldCheck } from "lucide-react";
import type { ChatPart, EnvAnswer, EnvRequestVar, EnvScope, PromptAnswer } from "../api";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { EscapeScope } from "./EscapeScope";
import { FileScopeContext } from "./FileReference";
import { Badge, Button, Input, MenuItem } from "./ui";

const SCOPES = ["project", "global"] as const;
const SCOPE_LABELS = {
  project: { inline: m.chat_env_this_project, menu: m.chat_env_save_to_project },
  global: { inline: m.chat_env_all_projects, menu: m.chat_env_save_for_all_projects },
};

/** The card an agent's `request_env` call shows: one field per variable, saved straight to the environment. */
export function EnvRequestCard({
  part,
  done,
  respond,
}: {
  part: ChatPart;
  done: boolean;
  respond: (answer: Omit<PromptAnswer, "promptId">) => Promise<void>;
}) {
  const prompt = part.prompt!;
  const variables = prompt.variables ?? [];
  if (prompt.resolved) return <EnvRequestRecord variables={variables} approved={prompt.approved} />;
  return <EnvRequestForm variables={variables} reason={prompt.question} done={done} respond={respond} />;
}

function EnvRequestForm({
  variables,
  reason,
  done,
  respond,
}: {
  variables: EnvRequestVar[];
  reason?: string;
  done: boolean;
  respond: (answer: Omit<PromptAnswer, "promptId">) => Promise<void>;
}) {
  const projectId = useContext(FileScopeContext)?.projectId;
  const { portalContainer } = useContext(WorkspaceDomContext);
  const saved = useQuery(useWorkspace().queries.getEnvVarsQuery(projectId)).data;
  const headingId = useId();

  const [values, setValues] = useState<ReadonlyMap<string, string>>(() => new Map());
  const [validation, setValidation] = useState<{ values: typeof values; variables: EnvRequestVar[]; mismatches: string[] }>();
  const [selectedScope, setScope] = useState<EnvScope>("project");
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    if (done) return;
    const fields = variables.flatMap(({ name, pattern }) => {
      const value = values.get(name)?.trim();
      return pattern && value ? [{ name, pattern, value }] : [];
    });
    if (!fields.length) return;

    let worker: Worker;
    try {
      worker = new Worker(new URL("../envPattern.worker.ts", import.meta.url), { type: "module" });
    } catch {
      return;
    }
    const timeout = window.setTimeout(() => stop(), 1000);
    function stop() {
      window.clearTimeout(timeout);
      worker.onmessage = null;
      worker.onerror = null;
      worker.terminate();
    }
    worker.onmessage = ({ data }: MessageEvent<string[]>) => {
      setValidation({ values, variables, mismatches: data });
      stop();
    };
    worker.onerror = stop;
    worker.postMessage(fields);
    return stop;
  }, [done, values, variables]);

  const scope = projectId ? selectedScope : "global";
  const scopeLabel = SCOPE_LABELS[scope].inline();
  const [beforeScope, afterScope] = m.chat_env_footer({ scope: "\0" }).split("\0");
  const savedVars = scope === "project" ? saved?.projectVars : saved?.vars;
  const mismatches = validation?.values === values && validation.variables === variables ? validation.mismatches : [];
  const answers: EnvAnswer[] = variables.flatMap(({ name }) => {
    const value = values.get(name)?.trim();
    return value ? [{ name, value, scope, mismatched: mismatches.includes(name) }] : [];
  });

  const save = () => { if (answers.length) void respond({ approve: true, env: answers }); };

  return (
    <form
      className={`prompt-card env my-3 w-full max-w-2xl overflow-hidden rounded-lg border border-border bg-background [&.readonly]:opacity-60 ${done ? "readonly" : ""}`}
      aria-labelledby={headingId}
      onSubmit={(event) => { event.preventDefault(); save(); }}
    >
      <div className="flex items-center gap-2.5 px-3.5 pt-3">
        <span className="flex size-7 shrink-0 items-center justify-center rounded-md bg-surface text-subtext">
          <KeyRound size={16} strokeWidth={1.5} aria-hidden="true" />
        </span>
        <span id={headingId} className="text-base font-semibold text-text">{m.chat_env_title()}</span>
      </div>
      {reason && <div dir="auto" className="px-3.5 pt-2 text-base leading-normal text-text wrap-anywhere">{reason}</div>}
      <div className="flex flex-col gap-4 px-3.5 py-3">
        {variables.map((variable, index) => {
          const descriptionId = `${headingId}-${index}-description`;
          const warningId = `${headingId}-${index}-warning`;
          const current = savedVars?.find((entry) => entry.key === variable.name);
          const value = values.get(variable.name) ?? "";
          const mismatched = mismatches.includes(variable.name);
          return (
            <div key={variable.name} className="flex flex-col gap-1.5">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
                <span className="font-mono text-sm text-text wrap-anywhere">{variable.name}</span>
                {current && <Badge>{m.chat_env_saved()}</Badge>}
              </div>
              {variable.description && <div id={descriptionId} dir="auto" className="text-sm leading-normal text-subtext wrap-anywhere">{variable.description}</div>}
              <Input
                type={variable.secret ? "password" : "text"}
                value={value}
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  setValues((all) => new Map(all).set(variable.name, value));
                }}
                placeholder={variable.example}
                aria-label={m.a11y_value_for({ name: ltr(variable.name) })}
                aria-invalid={mismatched || undefined}
                aria-describedby={[variable.description && descriptionId, mismatched && warningId].filter(Boolean).join(" ") || undefined}
                autoComplete={variable.secret ? "new-password" : "off"}
                spellCheck={false}
                disabled={done}
              />
              {mismatched && (
                <div id={warningId} className="text-sm text-accent-amber">
                  {variable.example ? m.chat_env_mismatch_example({ example: ltr(variable.example) }) : m.chat_env_mismatch()}
                </div>
              )}
              {current?.inProcessEnv && (
                <div className="text-sm text-subtext">{m.chat_env_shell_wins({ name: ltr(variable.name) })}</div>
              )}
            </div>
          );
        })}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border-variant px-3.5 py-2.5">
        <div className="me-auto flex min-w-0 items-start gap-2 text-sm leading-6 text-subtext">
          <ShieldCheck size={14} className="h-6 shrink-0" aria-hidden="true" />
          <p className="m-0 min-w-0">
            {beforeScope}
            {projectId ? (
              <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
                <Menu.Root modal={false} open={menuOpen && !done} onOpenChange={setMenuOpen}>
                  <Menu.Trigger
                    type="button"
                    disabled={done}
                    className="inline-block min-h-6 max-w-full cursor-pointer rounded-sm text-start text-text underline decoration-subtext decoration-dotted underline-offset-3 hover:decoration-solid data-popup-open:decoration-solid disabled:cursor-default disabled:no-underline focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
                  >
                    {scopeLabel}
                  </Menu.Trigger>
                  <Menu.Portal container={portalContainer}>
                    <Menu.Positioner className="z-50" positionMethod="fixed" side="top" align="start" sideOffset={4} collisionPadding={8}>
                      <Menu.Popup aria-label={m.chat_env_save_scope()} className="max-h-(--available-height) max-w-[calc(100vw-1rem)] overflow-y-auto rounded-lg border border-border bg-background p-1.5 shadow-menu outline-none">
                        <Menu.RadioGroup value={scope}>
                          {SCOPES.map((option) => (
                            <Menu.RadioItem
                              key={option}
                              value={option}
                              closeOnClick
                              nativeButton
                              render={<MenuItem size="compact" className="data-highlighted:bg-surface" />}
                              onClick={() => setScope(option)}
                            >
                              {SCOPE_LABELS[option].menu()}
                              <Menu.RadioItemIndicator className="shrink-0">
                                <Check size={14} aria-hidden="true" />
                              </Menu.RadioItemIndicator>
                            </Menu.RadioItem>
                          ))}
                        </Menu.RadioGroup>
                      </Menu.Popup>
                    </Menu.Positioner>
                  </Menu.Portal>
                  <EscapeScope enabled={menuOpen && !done} onEscape={() => setMenuOpen(false)} />
                </Menu.Root>
              </DirectionProvider>
            ) : scopeLabel}
            {afterScope}
          </p>
        </div>
        {!done && (
          <div className="flex gap-2">
            <Button type="button" size="small" variant="ghost" onClick={() => void respond({ approve: false })}>
              {m.chat_env_decline()}
            </Button>
            <Button type="submit" size="small" variant="primary" disabled={!answers.length}>
              {m.common_save()}
            </Button>
          </div>
        )}
      </div>
    </form>
  );
}

/** What an answered env card leaves in the transcript. */
function EnvRequestRecord({ variables, approved }: { variables: EnvRequestVar[]; approved?: boolean }) {
  const line = (variable: EnvRequestVar) => {
    const name = ltr(variable.name);
    if (variable.savedTo === "global") return m.chat_env_saved_global({ name });
    if (variable.savedTo === "project") return m.chat_env_saved_project({ name });
    return approved ? m.chat_env_left_unset({ name }) : null;
  };
  const lines = variables.map(line).filter((text) => text !== null);
  return (
    <div className="flex flex-col gap-1 py-1 text-base text-subtext">
      {approved === undefined ? (
        <div className="flex items-center gap-2"><KeyRound size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />{m.chat_env_ended()}</div>
      ) : approved === false ? (
        <div className="flex items-center gap-2"><KeyRound size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />{m.chat_env_declined()}</div>
      ) : lines.map((text) => (
        <div key={text} className="flex items-center gap-2"><KeyRound size={16} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />{text}</div>
      ))}
    </div>
  );
}
