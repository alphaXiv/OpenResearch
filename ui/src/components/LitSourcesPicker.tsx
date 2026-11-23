import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";
import { useRef } from "react";
import { Save } from "lucide-react";
import { Menu } from "@base-ui/react/menu";

import { m } from "../paraglide/messages.js";
import { mutSetChatLitSources, mutSetLitSources, mutUpdateProjectSettings } from "../queries/settings";
// Which sources an agent's discovery and paper reading may use. A chat follows its
// project and a project follows every project's choice, until either sets its own.

import { LitSourceLogo, LIT_SOURCE_NAME, type LitSource } from "./LitSourceLogo";
import { IconButton, MenuItem, SwitchIndicator, Tooltip, showAlert } from "./ui";

export const LIT_SOURCES: LitSource[] = ["alphaxiv", "openalex", "biorxiv", "pubmed"];

const showError = (error: unknown) => showAlert(error instanceof Error ? error.message : String(error), "error");

const sameSources = (a: readonly LitSource[], b: readonly LitSource[]) =>
  a.length === b.length && a.every((source) => b.includes(source));

const toggled = (disabled: readonly LitSource[], source: LitSource) =>
  disabled.includes(source) ? disabled.filter((off) => off !== source) : [...disabled, source];

/** The sources the project turned off, whether it chose them, and the choice for every project it otherwise follows. */
export function useProjectLitSources(projectId: string) {
  const { getLitSourcesQuery, getProjectSettingsQuery } = useWorkspace().queries;

  const machine = useQuery(getLitSourcesQuery());
  const project = useQuery(getProjectSettingsQuery(projectId));
  if (!machine.data || !project.data) return { error: machine.error ?? project.error ?? null, sources: null };

  const machineOff = LIT_SOURCES.filter((source) => !machine.data[source]);
  const own = project.data.disabledLitSources ?? null;
  return { error: null, sources: { disabled: own ?? machineOff, own: own !== null, machineOff } };
}

/**
 * A chat's sources. A draft has no chat yet, so its choice lives in `draft` until
 * the first message creates the chat with it; a side chat's draft starts from its parent's.
 */
export function useChatLitSources(
  projectId: string,
  sessionId: string | null,
  parentSessionId: string | null,
  draft: LitSource[] | null,
  setDraft: (disabled: LitSource[] | null) => void,
) {
  const workspace = useWorkspace();
  const { getChatLitSourcesQuery } = workspace.queries;
  const project = useProjectLitSources(projectId);
  const chat = useQuery({ ...getChatLitSourcesQuery(sessionId ?? ""), enabled: sessionId !== null });
  const parentId = sessionId ? null : parentSessionId;
  const parent = useQuery({ ...getChatLitSourcesQuery(parentId ?? ""), enabled: parentId !== null });
  const own = sessionId ? chat.data?.disabledLitSources ?? null : draft;
  if (!project.sources || (sessionId && !chat.data) || (parentId && !parent.data)) {
    return { error: project.error ?? chat.error ?? parent.error ?? null, sources: null };
  }

  const inherited = parent.data?.disabledLitSources ?? project.sources.disabled;
  const setOwn = (disabled: LitSource[] | null) => {
    if (sessionId) mutSetChatLitSources.run(workspace, sessionId, disabled);
    else setDraft(disabled);
  };
  return {
    error: null,
    sources: {
      disabled: own ?? inherited,
      differs: own !== null,
      toggle: (source: LitSource) => {
        const next = toggled(own ?? inherited, source);
        setOwn(sameSources(next, inherited) ? null : next);
      },
      revert: () => setOwn(null),
      // The chat keeps its own choice until the default it was saved as is saved.
      saveForProject: () => void mutUpdateProjectSettings
        .runAsHeadlessPromise(workspace, projectId, { disabledLitSources: own }, {})
        .then(() => setOwn(null), showError),
      saveForAll: () => {
        const disabled = own ?? inherited;
        const sources = Object.fromEntries(LIT_SOURCES.map((source) => [source, !disabled.includes(source)])) as Record<LitSource, boolean>;
        void mutSetLitSources.runAsHeadlessPromise(workspace, sources, {})
          .then(() => project.sources.own ? mutUpdateProjectSettings.runAsHeadlessPromise(workspace, projectId, { disabledLitSources: null }, {}) : undefined)
          .then(() => setOwn(null), showError);
      },
    },
  };
}

export function LitSourcesList({ disabled, onToggle }: { disabled: readonly LitSource[]; onToggle: (source: LitSource) => void }) {
  return (
    <div className="flex flex-col">
      {LIT_SOURCES.map((key) => {
        const on = !disabled.includes(key);
        return (
          <MenuItem key={key} type="button" role="switch" aria-checked={on} onClick={() => onToggle(key)}>
            <span className="inline-flex items-center gap-2">
              <LitSourceLogo source={key} size={16} decorative />
              {LIT_SOURCE_NAME[key]}
            </span>
            <SwitchIndicator checked={on} aria-hidden="true" />
          </MenuItem>
        );
      })}
    </div>
  );
}

/** The composer's sources menu: this chat's toggles, and while they differ from its project, a way to keep or drop them. */
export function ChatLitSources({ sources, error }: ReturnType<typeof useChatLitSources>) {
  // Inside the sources popover, so choosing an action doesn't count as a click outside it.
  const menuHost = useRef<HTMLDivElement>(null);
  return (
    <>
      <div className="flex min-h-7 items-center justify-between gap-2 ps-1">
        <span className="text-sm font-medium text-muted">{m.chat_panel_data_sources()}</span>
        {sources?.differs && (
          <Menu.Root modal={false}>
            <Tooltip interactive content={m.lit_sources_save_title()}>
              <Menu.Trigger render={<IconButton type="button" size="small" aria-label={m.lit_sources_save_title()} />}>
                <Save size={14} />
              </Menu.Trigger>
            </Tooltip>
            <Menu.Portal container={menuHost}>
              <Menu.Positioner positionMethod="fixed" side="bottom" align="end" sideOffset={4} collisionPadding={8} className="z-60">
                <Menu.Popup className="flex min-w-52 flex-col rounded-lg border border-border bg-background p-1 shadow-dropdown">
                  <Menu.Item render={<MenuItem type="button" />} onClick={sources.saveForProject}>{m.lit_sources_save_project()}</Menu.Item>
                  <Menu.Item render={<MenuItem type="button" />} onClick={sources.saveForAll}>{m.lit_sources_save_all()}</Menu.Item>
                  <Menu.Item render={<MenuItem type="button" className="text-subtext" />} onClick={sources.revert}>{m.lit_sources_revert()}</Menu.Item>
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        )}
      </div>
      {sources
        ? <LitSourcesList disabled={sources.disabled} onToggle={sources.toggle} />
        : <div role={error ? "alert" : undefined} className="py-1.5 px-2 text-muted text-sm">{error ? error.message : m.lit_sources_picker_loading()}</div>}
      <div ref={menuHost} />
    </>
  );
}
