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
  const clipboard = navigator.clipboard;
  if (!clipboard) {
    showAlert(m.file_tree_clipboard_unavailable(), "error");
    return;
  }
  void clipboard
    .writeText(`${root.replace(/[\\/]+$/, "")}/${path}`)
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
  | ReactMouseEvent<HTMLButtonElement>
  | ReactKeyboardEvent<HTMLButtonElement>;

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
  onClose,
}: {
  target: FileContextMenuTarget;
  onOpen: () => void;
  onRename?: () => void;
  onDuplicate?: () => void;
  onCopyPath: () => void;
  onDelete?: () => void;
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
    setPosition({
      x: Math.max(8, Math.min(target.x, window.innerWidth - menu.offsetWidth - 8)),
      y: Math.max(8, Math.min(target.y, window.innerHeight - menu.offsetHeight - 8)),
    });
    menu.querySelector<HTMLButtonElement>("button")?.focus();
    return () => {
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
    <MenuItem role="menuitem" danger={danger} onClick={() => run(action)}>
      <span>{label}</span>
    </MenuItem>
  );

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      aria-label={m.file_tree_file_actions({ path: ltr(target.path) })}
      className="option-menu fixed z-100 min-w-44 overflow-hidden rounded-lg border border-border bg-background p-1.5 shadow-menu"
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
      {item(m.file_tree_open(), onOpen)}
      {onRename && item(m.chat_panel_rename(), onRename)}
      {onDuplicate && item(m.file_tree_duplicate(), onDuplicate)}
      {item(m.artifacts_copy_path(), onCopyPath)}
      {onDelete && item(m.chat_panel_delete(), onDelete, true)}
    </div>,
    document.body,
  );
}
