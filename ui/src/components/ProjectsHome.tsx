import { m } from "../paraglide/messages.js";
import { useEffect, useRef } from "react";
import { type Project } from "../api";

import { NewProjectForm } from "./NewProjectForm";

export function NewProjectDialog({
  onClose,
  onCreated,
  remote = false,
}: {
  onClose: () => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  remote?: boolean;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      [...dialog.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
      )];
    (dialog.querySelector<HTMLElement>("[data-initial-focus]") ?? focusable()[0] ?? dialog).focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (
        event.key === "Enter" &&
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        event.shiftKey
      ) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = focusable();
      if (controls.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      previousFocus?.focus();
    };
  }, []);

  return (
    <div
      className="modal-backdrop fixed inset-0 bg-modal-backdrop flex items-start justify-center p-5 [--new-project-modal-top:clamp(4rem,20vh,24rem)] pt-[var(--new-project-modal-top)] overflow-y-auto z-100"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal w-120 max-w-full max-h-[calc(100vh_-_var(--new-project-modal-top)_-_1.25rem)] overflow-y-auto bg-background border border-border rounded-xl shadow-modal p-6 [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-xl [&_h2]:font-medium"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-project-dialog-title"
        tabIndex={-1}
      >
        <h2 id="new-project-dialog-title">{m.projects_home_new_project()}</h2>
        <NewProjectForm onCancel={onClose} onCreated={onCreated} remote={remote} />
      </div>
    </div>
  );
}

