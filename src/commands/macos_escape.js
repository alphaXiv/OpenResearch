window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    if (event.target?.matches?.("input[type=search]:enabled:not([readonly])")) return;
    // WebKit resends Escape to AppKit after its own dialog/popover dismissal, so dismiss here instead.
    event.preventDefault();
    // shortcut: last open popover stands in for the topmost; revisit if nested popovers render out of DOM order.
    const popover = CSS.supports("selector(:popover-open)") && [...document.querySelectorAll("[popover]:popover-open:not([popover=manual])")].pop();
    if (popover) return popover.hidePopover();
    // shortcut: old WebKit treats open dialogs as modal; revisit if non-modal dialogs are added.
    const dialog = [...document.querySelectorAll(CSS.supports("selector(:modal)") ? "dialog:modal" : "dialog[open]")].pop();
    if (dialog?.dispatchEvent(new Event("cancel", { cancelable: true }))) dialog.close();
});
