import { useEffect, useRef, useState, type FC, type HTMLInputTypeAttribute, type ReactNode } from "react";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Spinner,
  Textarea,
} from "./components/ui";
import { m } from "./paraglide/messages.js";

type Awaitable<T> = T | PromiseLike<T>;

/** All dialog showing functions accept these two functions */
export interface DialogOptions<FormResult> {
  /**
   * If this function is async, the dialog system will wait for it to complete
   * before closing the dialog. A throw keeps the dialog open and shows the error.
   */
  onConfirm: (result: FormResult) => Awaitable<void>;
  /**
   * "close" to do nothing besides close the dialog. To disable cancellation,
   * pass `"disable"`. This will make clicking outside the dialog or pressing
   * escape do nothing. A function may be async; the dialog closes once it
   * resolves, and a throw keeps the dialog open.
   */
  onCancel: (() => Awaitable<void>) | "disable" | "close";
}

export interface ConfirmDialogOptions {
  title: ReactNode;
  description: ReactNode;
  destructive?: boolean;
  confirmLabel?: string;
}

/**
 * Show a trivial confirmation dialog. Use this instead of rawdogging a trivial <Dialog/>
 */
export function showConfirmDialog(opts: ConfirmDialogOptions & DialogOptions<void>) {
  showCustomDialog({
    component: ConfirmDialogContent,
    onCancel: opts.onCancel,
    onConfirm: opts.onConfirm,
    props: {
      confirmLabel: opts.confirmLabel ?? (opts.onCancel === "disable" ? m.common_ok() : m.common_confirm()),
      description: opts.description,
      destructive: opts.destructive ?? false,
      title: opts.title,
    },
  });
}

export interface TextDialogOptions {
  title: ReactNode;
  description: ReactNode;
  placeholder?: string;
  initialValue?: string;
  destructive?: boolean;
  confirmLabel?: string;
  type?: HTMLInputTypeAttribute | "multiline";
  validateInput?: (str: string) => boolean;
}

/**
 * Show a single-field input dialog. Use this instead of rawdogging a trivial <Dialog/>
 */
export function showTextDialog(opts: TextDialogOptions & DialogOptions<string>) {
  showCustomDialog({
    component: TextDialogContent,
    onCancel: opts.onCancel,
    onConfirm: opts.onConfirm,
    props: {
      confirmLabel: opts.confirmLabel ?? m.common_submit(),
      description: opts.description,
      destructive: opts.destructive ?? false,
      title: opts.title,
      placeholder: opts.placeholder,
      initialValue: opts.initialValue,
      type: opts.type ?? "text",
      validateInput: opts.validateInput ?? ((value) => value.length > 0),
    },
  });
}

export interface CustomDialogOptions<Result, Props> {
  component: FC<DialogContentProps<Result> & Props>;
  props: Omit<Props, keyof DialogContentProps<Result>>;
}

/**
 * Show a customizable dialog, can be used to show a custom form or whatever
 * advanced UI you would like. You can use the `FormResult` template parameter to
 * hold form submit data. If you find yourself reaching for this component,
 * please consider if you can construct a wrapper API that makes sense.
 */
export function showCustomDialog<FormResult, Props>(opts: CustomDialogOptions<FormResult, Props> & DialogOptions<FormResult>) {
  if (!pushDialog) throw new Error("No <DialogManager /> mounted");
  const push = pushDialog;

  // requestAnimationFrame lets us avoid click lag spikes, which can cause the animation to skip sometimes.
  requestAnimationFrame(() => {
    push({
      id: crypto.randomUUID(),
      onCancel: opts.onCancel === "disable" ? null : opts.onCancel === "close" ? () => {} : opts.onCancel,
      // type-erasure for simplicity -- mixed-generic arrays are basically impossible
      component: opts.component as FC<DialogContentProps<unknown> & object>,
      props: opts.props,
      onConfirm: opts.onConfirm as (result: unknown) => Awaitable<void>,
      open: true,
      isDisabled: false,
      isPending: false,
      isError: false,
    });
  });
}

/** Opens a dialog titled `title` around arbitrary content; `render` receives `close`. */
export function showFormDialog(title: ReactNode, render: (close: () => void) => ReactNode) {
  showCustomDialog({
    component: FormDialogContent,
    props: { title, render },
    onConfirm: () => {},
    onCancel: "close",
  });
}

export interface DialogContentProps<Result> {
  confirm: (result: Result) => void;
  cancel: () => void;
  cancelable: boolean;
  isDisabled: boolean;
  isPending: boolean;
  errorMessage: string | null;
}

function ConfirmDialogContent({
  title,
  description,
  destructive,
  confirmLabel,
  cancel,
  confirm,
  cancelable,
  isDisabled,
  isPending,
  errorMessage,
}: DialogContentProps<void> & {
  title: ReactNode;
  description: ReactNode;
  destructive: boolean;
  confirmLabel: string;
}) {
  const confirmRef = useRef<HTMLButtonElement>(null);

  return (
    <DialogContent initialFocus={confirmRef}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {errorMessage != null && <DialogError>{errorMessage}</DialogError>}
      <DialogFooter>
        {cancelable && <Button onClick={cancel} disabled={isDisabled}>{m.common_cancel()}</Button>}
        <Button ref={confirmRef} variant={destructive ? "danger" : "primary"} disabled={isDisabled} onClick={() => confirm()}>
          {isPending && <Spinner />}
          {confirmLabel}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

function TextDialogContent({
  title,
  description,
  destructive = false,
  initialValue = "",
  placeholder,
  confirmLabel,
  cancel,
  confirm,
  cancelable,
  isDisabled,
  isPending,
  errorMessage,
  validateInput,
  type = "text",
}: DialogContentProps<string> & TextDialogOptions & { confirmLabel: string; validateInput: (str: string) => boolean }) {
  const [value, setValue] = useState(initialValue);
  const canSubmit = validateInput(value);

  return (
    <DialogContent
      render={<form action="#" onSubmit={(event) => {
        event.preventDefault();
        if (canSubmit) confirm(value);
      }} />}
    >
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {errorMessage != null && <DialogError>{errorMessage}</DialogError>}
      {type === "multiline" ? (
        <Textarea
          className="my-4 min-h-24"
          value={value}
          placeholder={placeholder}
          disabled={isDisabled}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }
          }}
        />
      ) : (
        <Input
          className="my-4"
          type={type}
          value={value}
          placeholder={placeholder}
          disabled={isDisabled}
          onChange={(event) => setValue(event.target.value)}
        />
      )}
      <DialogFooter>
        {cancelable && <Button type="button" onClick={cancel} disabled={isDisabled}>{m.common_cancel()}</Button>}
        <Button type="submit" variant={destructive ? "danger" : "primary"} disabled={!canSubmit || isDisabled}>
          {isPending && <Spinner />}
          {confirmLabel}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

function FormDialogContent({ title, render, cancel }: DialogContentProps<void> & { title: ReactNode; render: (close: () => void) => ReactNode }) {
  return (
    <DialogContent className="max-h-[85vh] w-lg overflow-y-auto">
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
      </DialogHeader>
      {render(cancel)}
    </DialogContent>
  );
}

function DialogError({ children }: { children: ReactNode }) {
  return <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{children}</p>;
}

interface DialogItem {
  id: string;
  open: boolean;
  isDisabled: boolean;
  isPending: boolean;
  error?: unknown;
  isError: boolean;
  component: FC<DialogContentProps<unknown> & object>;
  props: object;
  onConfirm: (result: unknown) => Awaitable<void>;
  onCancel: (() => Awaitable<void>) | null;
}

let pushDialog: ((item: DialogItem) => void) | null = null;

/** Mount once near the app root; `show*Dialog` throws without it. */
export function DialogManager() {
  const [dialogs, setDialogs] = useState<DialogItem[]>([]);
  const dialogsRef = useRef(dialogs);
  dialogsRef.current = dialogs;

  useEffect(() => {
    pushDialog = (item) => setDialogs((prev) => prev.concat(item));
    return () => { pushDialog = null; };
  }, []);

  const getDialog = (id: string) => dialogsRef.current.find((dialog) => dialog.id === id);

  const updateDialog = (id: string, patch: Partial<DialogItem>) =>
    setDialogs((prev) => prev.map((dialog) => (dialog.id === id ? { ...dialog, ...patch } : dialog)));

  const closeDialog = (id: string) => updateDialog(id, { open: false, isDisabled: true, isPending: false });

  const cancelDialog = (id: string) => {
    const dialog = getDialog(id);
    if (!dialog?.open || dialog.isDisabled) return;
    updateDialog(id, { isDisabled: true });
    new Promise<void>((resolve) => resolve(dialog.onCancel?.()))
      .then(() => closeDialog(id))
      .catch(() => updateDialog(id, { isDisabled: false }));
  };

  const confirmDialog = (id: string, value: unknown) => {
    const dialog = getDialog(id);
    if (!dialog?.open || dialog.isDisabled) return;
    updateDialog(id, { isDisabled: true, isPending: true, isError: false });
    new Promise<void>((resolve) => resolve(dialog.onConfirm(value)))
      .then(() => closeDialog(id))
      .catch((error) => {
        updateDialog(id, { isDisabled: false, isPending: false, isError: false });
        // Toggling isError off then on replays the shake on repeated failures.
        requestAnimationFrame(() => updateDialog(id, { isError: true, error }));
      });
  };

  return dialogs.map(({ id, open, isDisabled, isPending, component: Component, props, onCancel, isError, error }) => (
    <Dialog
      key={id}
      open={open}
      onOpenChange={onCancel && !isDisabled ? (nextOpen) => { if (!nextOpen) cancelDialog(id); } : undefined}
      onOpenChangeComplete={(nextOpen) => {
        if (!nextOpen) setDialogs((prev) => prev.filter((dialog) => dialog.id !== id));
      }}
      error={isError}
    >
      <Component
        {...props}
        confirm={(value: unknown) => confirmDialog(id, value)}
        cancel={() => cancelDialog(id)}
        isDisabled={isDisabled}
        isPending={isPending}
        errorMessage={isError ? (error instanceof Error ? error.message : String(error)) : null}
        cancelable={onCancel != null && !isDisabled}
      />
    </Dialog>
  ));
}
