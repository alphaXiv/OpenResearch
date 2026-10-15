import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { afterEach, expect, test, vi } from "vitest";
import type { ChatSession } from "../../src/api";
import { SessionRow } from "../../src/components/ChatPanel";
import { setLocale } from "../../src/paraglide/runtime.js";

function session(id: string, archived = false): ChatSession {
  return {
    id, title: id, projectId: "project", harness: "codex", model: null, serviceTier: null,
    permissionMode: null, planMode: false, autonomy: "copilot", reasoningLevel: null,
    archived, createdAt: 0, updatedAt: 0, busy: false, activeLeafId: null,
  };
}

function sidebar(archived = false) {
  const onOpen = vi.fn(), onRename = vi.fn(), onSetArchived = vi.fn(), onDelete = vi.fn();
  const menuBoundaryRef = createRef<HTMLElement>();
  const view = render(
    <>
      <section ref={menuBoundaryRef} data-testid="sidebar" style={{ overflow: "auto", height: 160 }}>
        {["First chat", "Older chat", "Following chat"].map((id, index) => (
          <div key={id} style={{ transform: `translateY(${index * 34}px)` }}>
            <SessionRow
              menuBoundaryRef={menuBoundaryRef}
              session={session(id, archived)} active={false} unread={false} busy={false} waiting={false}
              revealTitle={undefined} onOpen={() => onOpen(id)} onRename={(title) => onRename(id, title)}
              onSetArchived={(next) => onSetArchived(id, next)} onDelete={() => onDelete(id)}
            />
          </div>
        ))}
      </section>
      <section data-testid="transcript" style={{ overflow: "auto", height: 160 }}>Live reply</section>
      <button>Outside</button>
    </>,
  );
  const row = screen.getByText("Older chat").closest<HTMLElement>(".session-row")!;
  return { user: userEvent.setup(), unmount: view.unmount, row, trigger: within(row).getByRole("button", { name: "Session options" }), onOpen, onRename, onSetArchived, onDelete };
}

afterEach(() => setLocale("en", { reload: false }));

test("portals the expanded-sidebar menu outside transformed rows and renames only its owner", async () => {
  const { user, row, trigger, onOpen, onRename } = sidebar();
  await user.click(trigger);
  const menu = screen.getByRole("menu");
  expect(screen.getByTestId("sidebar").contains(menu)).toBe(false);
  expect(trigger.getAttribute("aria-expanded")).toBe("true");
  await user.click(within(menu).getByRole("menuitem", { name: "Rename" }));
  const input = within(row).getByRole("textbox", { name: "Session title" });
  await waitFor(() => expect(document.activeElement).toBe(input));
  expect(screen.getAllByRole("textbox")).toHaveLength(1);
  await user.clear(input);
  await user.type(input, "Renamed older chat{Enter}");
  expect(onRename.mock.calls).toEqual([["Older chat", "Renamed older chat"]]);
  expect(onOpen).not.toHaveBeenCalled();
  expect(screen.queryByRole("menu")).toBeNull();
});

test("an owner-row click only dismisses; a subsequent click can open the chat", async () => {
  const { user, trigger, onOpen } = sidebar();
  await user.click(trigger);
  await user.click(screen.getByText("Older chat"));
  expect(screen.queryByRole("menu")).toBeNull();
  expect(onOpen).not.toHaveBeenCalled();
  await user.click(screen.getByText("Older chat"));
  expect(onOpen.mock.calls).toEqual([["Older chat"]]);
});

test("keyboard Rename keeps focus in the inline editor and Escape cancels it", async () => {
  const { user, trigger, row, onRename, onOpen } = sidebar();
  trigger.focus();
  await user.keyboard("{Enter}{Enter}");
  const input = within(row).getByRole("textbox", { name: "Session title" });
  await waitFor(() => expect(document.activeElement).toBe(input));
  await user.keyboard("Unsaved{Escape}");
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.getByText("Older chat")).not.toBeNull();
  expect(onRename).not.toHaveBeenCalled();
  expect(onOpen).not.toHaveBeenCalled();
});

test("keyboard navigation, Escape dismissal and trigger focus are managed by the menu", async () => {
  const { user, trigger, onDelete } = sidebar();
  trigger.focus();
  await user.keyboard("{ArrowDown}");
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Rename" })));
  await user.keyboard("{ArrowDown}");
  expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Archive" }));
  await user.keyboard("{End}");
  expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Delete" }));
  await user.keyboard("{ArrowDown}");
  expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Rename" }));
  const pageEscape = vi.fn();
  document.addEventListener("keydown", pageEscape);
  try {
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(trigger));
    expect(pageEscape).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
  } finally {
    document.removeEventListener("keydown", pageEscape);
  }
});

test("outside interactions and the trigger toggle dismiss without selecting a chat", async () => {
  const { user, trigger, onOpen } = sidebar();
  await user.click(trigger);
  await user.click(screen.getByRole("button", { name: "Outside" }));
  expect(screen.queryByRole("menu")).toBeNull();
  await user.click(trigger);
  await user.click(trigger);
  expect(screen.queryByRole("menu")).toBeNull();
  expect(onOpen).not.toHaveBeenCalled();
});

test.each([false, true])("archive action uses the intended session and toggles archived=%s", async (archived) => {
  const { user, trigger, onSetArchived, onOpen } = sidebar(archived);
  await user.click(trigger);
  await user.click(screen.getByRole("menuitem", { name: archived ? "Unarchive" : "Archive" }));
  expect(onSetArchived.mock.calls).toEqual([["Older chat", !archived]]);
  expect(onOpen).not.toHaveBeenCalled();
  expect(screen.queryByRole("menu")).toBeNull();
});

test("keyboard activation deletes only the intended session", async () => {
  const { user, trigger, onDelete, onOpen } = sidebar();
  trigger.focus();
  await user.keyboard("{Enter}{End}{Enter}");
  expect(onDelete.mock.calls).toEqual([["Older chat"]]);
  expect(onOpen).not.toHaveBeenCalled();
  expect(screen.queryByRole("menu")).toBeNull();
});

test("unrelated transcript scrolling does not dismiss the menu", async () => {
  const { user, trigger } = sidebar();
  await user.click(trigger);
  fireEvent.scroll(screen.getByTestId("transcript"));
  expect(screen.queryByRole("menu")).not.toBeNull();
});

test("sidebar scrolling hides a fully clipped but still mounted overscanned owner menu", async () => {
  const { user, trigger } = sidebar();
  await user.click(trigger);
  expect(screen.queryByRole("menu")).not.toBeNull();
  const bounds = vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(new DOMRect(100, 50, 16, 16));
  try {
    fireEvent.scroll(screen.getByTestId("sidebar"));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(screen.getByText("Older chat").isConnected).toBe(true);
    const wrapper = screen.getByRole("menu", { hidden: true }).parentElement!;
    expect(wrapper.style.visibility).toBe("hidden");
    expect(wrapper.style.pointerEvents).toBe("none");
  } finally {
    bounds.mockRestore();
  }
});

test("unmounting a virtualized owner row removes its portal", async () => {
  const { user, trigger, unmount } = sidebar();
  await user.click(trigger);
  expect(screen.queryByRole("menu")).not.toBeNull();
  unmount();
  expect(screen.queryByRole("menu")).toBeNull();
});

test("the shared menu passes RTL direction to its positioning and navigation primitive", async () => {
  const { user, trigger } = sidebar();
  setLocale("ar", { reload: false });
  await user.click(trigger);
  expect(screen.getByRole("menu").getAttribute("dir")).toBe("rtl");
});
