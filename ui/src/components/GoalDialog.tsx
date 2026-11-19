import { useQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { Button, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Spinner, Textarea } from "./ui";

/** Edits a chat's goal; confirms with the new goal, or null to clear it. */
export function GoalDialog({
  sessionId,
  projectId,
  initialGoal,
  focusComposer,
  confirm,
  cancel,
  cancelable,
  isDisabled,
  isPending,
  errorMessage,
}: DialogContentProps<string | null> & { sessionId: string; projectId: string; initialGoal: string; focusComposer: () => void }) {
  const { listChatSessionsQuery } = useWorkspace().queries;
  const { data: sessions } = useQuery(listChatSessionsQuery(projectId));
  const session = sessions?.find((item) => item.id === sessionId);
  const saved = session ? session.goal?.trim() ?? "" : undefined;

  // The saved goal the draft started from; the agent or another window may change it meanwhile.
  const [base, setBase] = useState(initialGoal);
  const [draft, setDraft] = useState(initialGoal);
  const wanted = draft.trim();
  const changed = saved !== undefined && saved !== base && saved !== wanted;
  const field = useRef<HTMLTextAreaElement>(null);

  return (
    <DialogContent
      // Without a goal the chip that opened the dialog is gone.
      finalFocus={() => {
        if (saved !== "") return true;
        focusComposer();
        return false;
      }}
      render={
        <form
          action="#"
          onSubmit={(event) => {
            event.preventDefault();
            if (wanted) confirm(wanted);
          }}
        />
      }
    >
      <DialogHeader>
        <DialogTitle>{m.goal_dialog_title()}</DialogTitle>
        <DialogDescription>{m.goal_dialog_description()}</DialogDescription>
      </DialogHeader>
      {errorMessage != null && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
      <Textarea
        ref={field}
        className="mt-4 min-h-28"
        aria-label={m.chat_panel_goal()}
        value={draft}
        disabled={isDisabled}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.form?.requestSubmit();
          }
        }}
      />
      <div role="status">
        {changed && (
          <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-md bg-accent-amber-subtle px-3 py-2 text-sm text-text">
            <span className="min-w-0 flex-1">{saved ? m.goal_dialog_changed() : m.goal_dialog_cleared()}</span>
            {saved && (
              <Button type="button" size="small" disabled={isDisabled} onClick={() => {
                setBase(saved);
                setDraft(saved);
                field.current?.focus();
              }}>
                {m.goal_dialog_load_saved()}
              </Button>
            )}
          </div>
        )}
      </div>
      <DialogFooter>
        {saved !== "" && (
          <Button type="button" variant="danger" className="me-auto" disabled={isDisabled} onClick={() => confirm(null)}>
            {m.goal_dialog_clear()}
          </Button>
        )}
        {cancelable && (
          <Button type="button" onClick={cancel} disabled={isDisabled}>
            {m.common_cancel()}
          </Button>
        )}
        <Button type="submit" variant="primary" disabled={!wanted || isDisabled}>
          {isPending && <Spinner />}
          {m.common_save()}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}
