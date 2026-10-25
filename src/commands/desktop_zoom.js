(() => {
    const mac = "__ORX_ZOOM_PLATFORM__" === "macos";
    const main = window === window.top;
    if (main) {
        window.addEventListener("message", ({ data }) => {
            if (data === "page-zoom:in" || data === "page-zoom:out" || data === "page-zoom:reset") {
                window.ipc.postMessage(data);
            }
        });
        // Restore titlebar spacing as soon as the new document is ready after a reload.
        document.addEventListener("DOMContentLoaded", () => {
            window.ipc.postMessage("page-zoom:sync");
        }, { once: true });
    }
    window.addEventListener("keydown", (event) => {
        // macOS uses native menu accelerators.
        if (mac || !event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return;
        const action = event.key === "=" || event.key === "+" ? "in"
            : event.key === "-" ? "out"
            : event.key === "0" && !event.shiftKey ? "reset" : null;
        if (!action) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        // Sandboxed previews cannot access the main frame's IPC directly.
        if (main) window.ipc.postMessage(`page-zoom:${action}`);
        else window.top.postMessage(`page-zoom:${action}`, "*");
    }, true);
})();
