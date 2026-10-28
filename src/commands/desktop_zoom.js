(() => {
    const mac = "__ORX_ZOOM_PLATFORM__" === "macos";
    const main = window === window.top;
    if (main) {
        window.addEventListener("message", ({ data }) => {
            if (data === "orx:command:tab.close") {
                window.dispatchEvent(new CustomEvent("orx:command", { detail: "tab.close" }));
                return;
            }
            if (data === "page-zoom:in" || data === "page-zoom:out" || data === "page-zoom:reset") {
                window.ipc.postMessage(data);
            }
        });
        // Restore titlebar spacing when a new document is ready.
        document.addEventListener("DOMContentLoaded", () => {
            window.ipc.postMessage("page-zoom:sync");
        }, { once: true });
    }
    window.addEventListener("keydown", (event) => {
        // Main-frame shortcuts belong to TanStack; macOS uses native menu accelerators.
        if (main || mac || !event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return;
        if (event.key.toLowerCase() === "w" && !event.shiftKey) {
            event.preventDefault();
            event.stopImmediatePropagation();
            window.top.postMessage("orx:command:tab.close", "*");
            return;
        }
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
