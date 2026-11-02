import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { Menu } from "@base-ui/react/menu";
import { DirectionProvider } from "@base-ui/react/direction-provider";
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
  useEffect(() => {
    const close = () => onCloseRef.current();
    const dismissOnScroll = (event: Event) => {
      if (event.target instanceof Node && menuRef.current?.contains(event.target)) return;
      close();
    };
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", dismissOnScroll, true);
    return () => {
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", dismissOnScroll, true);
    };
  }, []);

  return (
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root open modal={false} onOpenChange={(open) => { if (!open) onClose(); }}>
        <Menu.Portal>
          <Menu.Positioner
            positionMethod="fixed"
            anchor={{ getBoundingClientRect: () => new DOMRect(target.x, target.y, 0, 0) }}
            side="bottom"
            align="start"
            collisionPadding={8}
            className="z-100"
          >
            <Menu.Popup
              ref={menuRef}
              aria-label={m.file_tree_file_actions({ path: ltr(target.path) })}
              className="outline-none option-menu min-w-44 max-h-(--available-height) overflow-y-auto rounded-md border border-border bg-background p-1 shadow-menu"
              onContextMenu={(event) => event.preventDefault()}
            >
              <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={onOpen}>{m.file_tree_open()}</Menu.Item>
              {onRename && <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={onRename}>{m.chat_panel_rename()}</Menu.Item>}
              {onDuplicate && <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={onDuplicate}>{m.file_tree_duplicate()}</Menu.Item>}
              <Menu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={onCopyPath}>{m.artifacts_copy_path()}</Menu.Item>
              {onDelete && <Menu.Item nativeButton render={<MenuItem size="compact" danger className="data-highlighted:bg-surface" />} onClick={onDelete}>{m.chat_panel_delete()}</Menu.Item>}
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
  );
}
