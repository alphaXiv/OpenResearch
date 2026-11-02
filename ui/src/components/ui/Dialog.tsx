import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { createContext, useContext, useEffect, useState, type ComponentProps } from "react";
import { EscapeScope } from "../EscapeScope";
import { cn } from "./cn";

type StringClass<T> = Omit<T, "className"> & { className?: string };

const DialogErrorContext = createContext(false);

/** `error` replays a shake on the popup; toggle it false then true to retrigger. */
export function Dialog({ error = false, open, children, ...props }: BaseDialog.Root.Props & { error?: boolean }) {
  // Base UI only plays the enter transition on a closed-to-open change, so a dialog mounted with `open` opens a frame later.
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    const frame = requestAnimationFrame(() => setMounted(true));
    return () => cancelAnimationFrame(frame);
  }, []);

  return (
    <DialogErrorContext value={error}>
      <EscapeScope onEscape={() => {}} enabled={open}>
        <BaseDialog.Root open={open && mounted} {...props}>{children}</BaseDialog.Root>
      </EscapeScope>
    </DialogErrorContext>
  );
}

export function DialogContent({ className, children, ...props }: StringClass<BaseDialog.Popup.Props>) {
  const error = useContext(DialogErrorContext);
  return (
    <BaseDialog.Portal>
      <BaseDialog.Backdrop className="fixed inset-0 z-200 bg-modal-backdrop transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
      <BaseDialog.Viewport className="fixed inset-0 z-200 flex items-center justify-center p-5">
        <BaseDialog.Popup
          className={cn(
            "w-120 max-w-full rounded-xl border border-border bg-background p-6 shadow-modal outline-none",
            "transition-[opacity,scale] duration-150 ease-standard data-ending-style:scale-97 data-ending-style:opacity-0 data-starting-style:scale-97 data-starting-style:opacity-0",
            error && "motion-safe:animate-dialog-shake",
            className,
          )}
          {...props}
        >
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Viewport>
    </BaseDialog.Portal>
  );
}

export function DialogHeader({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("flex flex-col gap-2", className)} {...props} />;
}

export function DialogTitle({ className, ...props }: StringClass<BaseDialog.Title.Props>) {
  return <BaseDialog.Title className={cn("m-0 text-xl font-medium text-text", className)} {...props} />;
}

export function DialogDescription({ className, ...props }: StringClass<BaseDialog.Description.Props>) {
  return <BaseDialog.Description className={cn("m-0 text-sm text-subtext", className)} {...props} />;
}

export function DialogFooter({ className, ...props }: ComponentProps<"div">) {
  return <div className={cn("mt-6 flex justify-end gap-2.5", className)} {...props} />;
}
