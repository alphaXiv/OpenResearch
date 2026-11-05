import { useQuery } from "@tanstack/react-query";
import { Copy, Cpu, Download, FoldVertical, Goal, History, ListChecks, MessagesSquare, SquarePen, WandSparkles, type LucideIcon } from "lucide-react";

import { m } from "../paraglide/messages.js";
import { Fragment, useContext, useLayoutEffect, useState, type CSSProperties, type ReactNode } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { createPortal } from "react-dom";
import { type SkillInfo } from "../api";
import { CITED_ID } from "../agentMessages";
import { PAPER_LINK } from "../fileMentions";
import { CitedRef, PaperPill } from "./ChatLinks";
import { commandDisplayName, isComposerCommand, splitCommandTokens, type ComposerCommandName } from "../composerCommands";
import { Md } from "./Md";
import { Badge } from "./ui";

const COMMAND_ICONS: Record<ComposerCommandName, LucideIcon> = {
  plan: ListChecks,
  goal: Goal,
  new: SquarePen,
  resume: History,
  model: Cpu,
  compact: FoldVertical,
  copy: Copy,
  export: Download,
  side: MessagesSquare,
};

/** Skills never share a command's name (see `commandsForHarness`), so the name alone picks the icon. */
export function CommandIcon({ name, className }: { name: string; className?: string }) {
  const Icon = isComposerCommand(name) ? COMMAND_ICONS[name] : WandSparkles;
  return <Icon size={16} strokeWidth={1.5} className={className} aria-hidden="true" />;
}

function SkillLabel({ name }: { name: string }) {
  return (
    <>
      <CommandIcon name={name} className="me-1 inline-block align-middle" />
      {commandDisplayName(name)}
    </>
  );
}

const CITED_CONTEXT = new RegExp(`${PAPER_LINK.source}|${CITED_ID.source}`, "g");

/** Papers, message refs and chat ids in a user's message, as links to them. */
function withCitedContext(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(CITED_CONTEXT)) {
    const { paperTitle, paperUrl } = match.groups ?? {};
    nodes.push(
      text.slice(last, match.index),
      paperTitle && paperUrl
        ? <PaperPill key={match.index} title={paperTitle} url={paperUrl} />
        : <CitedRef key={match.index} cited={match[0]} fallback={match[0]} />,
    );
    last = match.index + match[0].length;
  }
  nodes.push(text.slice(last));
  return nodes;
}

function chipSegments(text: string, isCommand: (name: string) => boolean): ReactNode[] {
  return splitCommandTokens(text, isCommand).map((segment, i, segments) => {
    if (segment.command) {
      return (
        <span key={i} className="skill-chip me-0.5 whitespace-nowrap font-normal text-skill-blue">
          <SkillLabel name={segment.text.slice(1).toLowerCase()} />
        </span>
      );
    }
    let plainText = segment.text;
    if (segments[i - 1]?.command) plainText = plainText.replace(/^[ \t]+/, " ");
    if (segments[i + 1]?.command) plainText = plainText.replace(/[ \t]+$/, " ");
    return <Fragment key={i}>{withCitedContext(plainText)}</Fragment>;
  });
}

/** A skill's instructions, previewed beside the composer chip `anchor`. */
export function SkillPreviewCard({
  id,
  anchor,
  name,
  skill,
  projectId,
  onMouseEnter,
  onMouseLeave,
}: {
  id: string;
  anchor: HTMLElement;
  name: string;
  skill: SkillInfo;
  projectId: string;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const { getSkillContentQuery } = useWorkspace().queries;
  const preview = useQuery(getSkillContentQuery(name, projectId, skill.harness));
  const content = preview.data ?? null;
  const [position, setPosition] = useState<CSSProperties>({});

  useLayoutEffect(() => {
    const place = () => {
      const rect = anchor.getBoundingClientRect();
      const width = Math.min(420, window.innerWidth - 32);
      const left = Math.max(16, Math.min(rect.left - 4, window.innerWidth - width - 16));
      setPosition(
        rect.top > 300
          ? { bottom: window.innerHeight - rect.top + 12, left, width }
          : { left, top: rect.bottom + 12, width },
      );
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor]);

  return createPortal(
    <div
      id={id}
      role="dialog"
      aria-label={m.a11y_skill({ name })}
      style={{ ...position, maxHeight: "min(28rem, calc(100vh - 2rem))" }}
      className="fixed z-100 overflow-y-auto rounded-lg border border-border bg-background shadow-dropdown"
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <div className="sticky top-0 z-1 flex items-center gap-2 border-b border-border-variant bg-background px-4 py-3">
        <span className="text-sm font-medium text-muted">{commandDisplayName(name)}</span>
        <Badge className="h-5 border-border-variant bg-surface px-1.5 tracking-wider">
          {m.skill_chips_badge()}
        </Badge>
      </div>
      <div className="p-4 text-sm text-text">
        {preview.isFetching && content === null ? (
          <span className="text-muted">{m.skill_chips_loading_skill()}</span>
        ) : (
          <Md text={content ?? skill.description} />
        )}
      </div>
    </div>,
    portalContainer ?? document.body,
  );
}

/** A sent message's text with every known `/command` rendered as a chip. */
export function MessageWithChips({
  text,
  isCommand,
}: {
  text: string;
  isCommand: (name: string) => boolean;
}) {
  return <>{chipSegments(text, isCommand)}</>;
}
