import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

vi.stubGlobal("PointerEvent", MouseEvent);
vi.stubGlobal("ResizeObserver", class {
  observe() {}
  unobserve() {}
  disconnect() {}
});
HTMLElement.prototype.hasPointerCapture = () => false;
HTMLElement.prototype.setPointerCapture = () => {};
HTMLElement.prototype.releasePointerCapture = () => {};
HTMLElement.prototype.scrollIntoView = () => {};
window.matchMedia = (media) => ({
  matches: false, media, onchange: null,
  addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {},
  dispatchEvent: () => false,
});
vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);

// jsdom has no layout; give Radix a visible anchor and viewport. Browser verification covers geometry.
vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(100, 100, 140, 30));
Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 140 });
Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 160 });
Object.defineProperty(document.documentElement, "clientWidth", { value: 1024 });
Object.defineProperty(document.documentElement, "clientHeight", { value: 768 });

afterEach(cleanup);
