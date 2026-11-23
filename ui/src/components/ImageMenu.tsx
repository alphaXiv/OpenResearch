import { ContextMenu } from "@base-ui/react/context-menu";
import { useContext, useRef, type ReactElement } from "react";
import { copyImageToClipboard } from "../clipboard";
import { downloadUrl } from "../download";
import { m } from "../paraglide/messages.js";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { MenuItem, MENU_POPUP, openContextMenuFromKeyboard } from "./ui/MenuItem";

function shownImage(element: HTMLElement | null) {
  return element instanceof HTMLImageElement ? element : element?.querySelector("img") ?? null;
}

/** Replaces the webview's image menu on `children`, an image or the control wrapping one. */
export function ImageMenu({ src, name, children }: { src: string; name: string; children: ReactElement }) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const trigger = useRef<HTMLElement>(null);
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger render={children} ref={(element: HTMLElement | null) => { trigger.current = element; }}
        onKeyDown={openContextMenuFromKeyboard} aria-keyshortcuts="Shift+F10 ContextMenu" />
      <ContextMenu.Portal container={portalContainer}>
        {/* Above the z-200 dialogs, for the image viewer. */}
        <ContextMenu.Positioner className="z-210" collisionPadding={8}>
          <ContextMenu.Popup finalFocus={trigger} aria-label={m.image_menu()} className={`${MENU_POPUP} min-w-44 max-h-(--available-height)`}
            onContextMenu={(event) => event.preventDefault()}>
            <ContextMenu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />} onClick={() => {
              const image = shownImage(trigger.current);
              if (image) void copyImageToClipboard(image);
            }}>{m.image_copy()}</ContextMenu.Item>
            <ContextMenu.Item nativeButton render={<MenuItem size="compact" className="data-highlighted:bg-surface" />}
              onClick={() => downloadUrl(name, src)}>{m.image_save()}</ContextMenu.Item>
          </ContextMenu.Popup>
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}
