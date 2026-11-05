import type { ChatTextAnnotation, TurnOptions } from "./api";

/** Per-chat stash for unsent composer content (draft, attachments, annotations). */
export interface ComposerAttachment {
  /** Unique across composers; the draft cites it as `[Image #id]` or `[PDF #id]`. */
  id: number;
  dataUrl: string;
  mediaType: string;
  name?: string;
  size: number;
}

let lastAttachmentId = 0;

export function nextAttachmentId(): number {
  return ++lastAttachmentId;
}

export function attachmentToken(attachment: Pick<ComposerAttachment, "id" | "mediaType">): string {
  return `[${attachment.mediaType === "application/pdf" ? "PDF" : "Image"} #${attachment.id}]`;
}

export const ATTACHMENT_TOKEN = /\[(?:Image|PDF) #(\d+)\]/g;

/** The attachments a draft cites, in order of first mention, with each token
 * renumbered from 1 so the text and the attachment list agree. Uncited
 * attachments were deleted from the text and are dropped. */
export function citedAttachments(
  draft: string,
  attachments: ComposerAttachment[],
): { text: string; attachments: ComposerAttachment[] } {
  const cited: ComposerAttachment[] = [];
  const text = draft.replace(ATTACHMENT_TOKEN, (token, id: string) => {
    const attachment = attachments.find((candidate) => candidate.id === Number(id));
    if (!attachment) return token;
    if (!cited.includes(attachment)) cited.push(attachment);
    return attachmentToken({ ...attachment, id: cited.indexOf(attachment) + 1 });
  });
  return { text, attachments: cited };
}

export interface ComposerAnnotation extends ChatTextAnnotation {
  id: string;
  range?: Range;
}

export interface ComposerStash {
  draft: string;
  attachments: ComposerAttachment[];
  annotations: ComposerAnnotation[];
  unconfirmedSend: string | null;
  pendingClientTurn: { signature: string; id: string; mode: "steer" | null; options: TurnOptions; invalidated?: boolean } | null;
}

export const EMPTY_COMPOSER_STASH: ComposerStash = {
  draft: "",
  attachments: [],
  annotations: [],
  unconfirmedSend: null,
  pendingClientTurn: null,
};

/** Annotation Ranges reference transcript DOM that won't exist on return. */
export function composerStashContent(
  live: ComposerStash,
  prefillDraft: string | null,
): ComposerStash | null {
  const emptyDraft = live.draft === "" || live.draft === prefillDraft;
  if (emptyDraft && live.attachments.length === 0 && live.annotations.length === 0 && live.unconfirmedSend === null) {
    return null;
  }
  return {
    ...live,
    annotations: live.annotations.map(({ range: _range, ...annotation }) => annotation),
  };
}

/** A failed send's content returned to a composer that may have moved on: the failed draft leads. */
export function mergeFailedComposer(current: ComposerStash, failed: ComposerStash): ComposerStash {
  const annotations = [...current.annotations];
  for (const annotation of failed.annotations) {
    if (annotations.some((item) => item.id === annotation.id && item.text === annotation.text)) continue;
    let id = annotation.id;
    while (annotations.some((item) => item.id === id)) id += ":recovered";
    annotations.unshift(id === annotation.id ? annotation : { ...annotation, id });
  }
  return {
    draft: [failed.draft, current.draft].filter(Boolean).join("\n\n"),
    attachments: [...failed.attachments.filter((item) => !current.attachments.includes(item)), ...current.attachments],
    annotations,
    unconfirmedSend: current.unconfirmedSend ?? failed.unconfirmedSend,
    pendingClientTurn: current.pendingClientTurn ?? failed.pendingClientTurn,
  };
}

export function createComposerState() {
  const composerSeeds = new Map<string, string>();
  return {
    composerStashes: new Map<string, ComposerStash>(),
    mountedComposers: new Map<string, (failed: ComposerStash) => void>(),
    seedComposer(projectId: string, prompt: string) {
      composerSeeds.set(projectId, prompt);
    },
    composerSeed(projectId: string) {
      return composerSeeds.get(projectId) ?? null;
    },
    forgetComposerSeed(projectId: string) {
      composerSeeds.delete(projectId);
    },
  };
}
