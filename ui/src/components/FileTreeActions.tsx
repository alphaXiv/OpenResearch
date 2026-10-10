import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { createPortal } from "react-dom";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { Input, MenuItem, showAlert } from "./ui";

export function copyFilePath(root: string, path: string) {
  copyAbsoluteFilePath(path ? `${root.replace(/[\\/]+$/, "")}/${path}` : root);
}

export function copyAbsoluteFilePath(path: string | Promise<string>) {
  const clipboard = navigator.clipboard;
  if (!clipboard) {
    if (typeof path !== "string") void path.catch(() => {});
    showAlert(m.file_tree_clipboard_unavailable(), "error");
    return;
  }
  // WebKit requires the write to begin inside the user gesture, even when
  // canonicalizing a tree entry needs an asynchronous server round trip.
  const write = typeof path === "string" ? clipboard.writeText(path)
    : typeof ClipboardItem !== "undefined" && clipboard.write
      ? clipboard.write([new ClipboardItem({ "text/plain": path.then(value => new Blob([value], { type: "text/plain" })) })])
      : path.then(value => clipboard.writeText(value));
  void write
    .then(() => showAlert(m.common_copied(), "success"))
    .catch((error) => showAlert(error instanceof Error ? error.message : String(error), "error"));
}

export function FileRenameInput({
  name,
  onCommit,
  onCancel,
}: {
  name: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState(name);
  const finished = useRef(false);

  const finish = () => {
    if (finished.current) return;
    finished.current = true;
    const next = draft.trim();
    if (!next || next === name) onCancel();
    else onCommit(next);
  };

  return (
    <Input
      autoFocus
      variant="inline"
      className="min-w-0 flex-1"
      value={draft}
      aria-label={m.file_tree_rename_file({ path: ltr(name) })}
      onFocus={(event) => {
        const extension = name.lastIndexOf(".");
        event.currentTarget.setSelectionRange(0, extension > 0 ? extension : name.length);
      }}
      onChange={(event) => setDraft(event.target.value)}
      onClick={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onBlur={finish}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") {
          event.preventDefault();
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          event.preventDefault();
          finished.current = true;
          onCancel();
        }
      }}
    />
  );
}

export interface FileContextMenuTarget {
  path: string;
  x: number;
  y: number;
}

export type FileContextMenuEvent =
  | ReactMouseEvent<HTMLElement>
  | ReactKeyboardEvent<HTMLElement>;

export function fileContextMenuTarget(
  event: FileContextMenuEvent,
  path: string,
): FileContextMenuTarget {
  const rect = event.currentTarget.getBoundingClientRect();
  const x = "clientX" in event ? event.clientX : 0;
  const y = "clientY" in event ? event.clientY : 0;
  return {
    path,
    x: x || rect.left + 16,
    y: y || rect.top + rect.height,
  };
}

export function FileContextMenu({
  target,
  onOpen,
  onRename,
  onDuplicate,
  onCopyPath,
  onDelete,
  onReveal,
  pathStatus,
  copyLabel,
  openLabel,
  onClose,
}: {
  target: FileContextMenuTarget;
  onOpen?: () => void;
  onRename?: () => void;
  onDuplicate?: () => void;
  onCopyPath?: () => void;
  onDelete?: () => void;
  onReveal?: () => void;
  pathStatus?: string;
  copyLabel?: string;
  openLabel?: string;
  onClose: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const [position, setPosition] = useState({ x: target.x, y: target.y });

  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const previousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const reposition = () => setPosition({
      x: Math.max(8, Math.min(target.x, window.innerWidth - menu.offsetWidth - 8)),
      y: Math.max(8, Math.min(target.y, window.innerHeight - menu.offsetHeight - 8)),
    });
    reposition();
    // Resolving a chat link adds its path and OS actions after the menu opens.
    const observer = new ResizeObserver(reposition);
    observer.observe(menu);
    menu.querySelector<HTMLButtonElement>("button")?.focus();
    return () => {
      observer.disconnect();
      if (menu.contains(document.activeElement)) previousFocus?.focus();
    };
  }, [target]);

  useEffect(() => {
    const close = () => onCloseRef.current();
    const dismiss = (event: Event) => {
      if (!menuRef.current?.contains(event.target instanceof Node ? event.target : null)) onCloseRef.current();
    };
    const keydown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Tab") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onCloseRef.current();
    };
    document.addEventListener("pointerdown", dismiss);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    document.addEventListener("keydown", keydown, true);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
      document.removeEventListener("keydown", keydown, true);
    };
  }, []);

  const run = (action: () => void) => {
    onCloseRef.current();
    action();
  };
  const item = (label: string, action: () => void, danger = false) => (
    <MenuItem size="compact" role="menuitem" danger={danger} onClick={() => run(action)}>
      <span>{label}</span>
    </MenuItem>
  );

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      aria-label={m.file_tree_file_actions({ path: ltr(target.path) })}
      className="option-menu fixed z-100 min-w-44 overflow-hidden rounded-md border border-border bg-background p-1 shadow-menu"
      style={{ left: position.x, top: position.y }}
      onContextMenu={(event) => event.preventDefault()}
      onKeyDown={(event) => {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        const items = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
        const current = items.indexOf(document.activeElement instanceof HTMLButtonElement ? document.activeElement : items[0]);
        const step = event.key === "ArrowDown" ? 1 : -1;
        items[(current + step + items.length) % items.length]?.focus();
      }}
    >
      {pathStatus && <p role="status" className="m-0 max-w-80 break-all px-2 py-1 text-xs text-subtext" dir="auto">{pathStatus}</p>}
      {onOpen && item(openLabel ?? m.file_tree_open(), onOpen)}
      {onRename && item(m.chat_panel_rename(), onRename)}
      {onDuplicate && item(m.file_tree_duplicate(), onDuplicate)}
      {onCopyPath && item(copyLabel ?? m.artifacts_copy_path(), onCopyPath)}
      {onReveal && item(m.file_viewer_reveal_in_file_manager(), onReveal)}
      {onDelete && item(m.chat_panel_delete(), onDelete, true)}
    </div>,
    document.body,
  );
}
