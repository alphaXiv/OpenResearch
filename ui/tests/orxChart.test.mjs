import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const html = readFileSync(new URL("../../agent-skills/orx-figures/assets/orx-chart.html", import.meta.url), "utf8");
const script = html.split("<script>")[1].split("</script>")[0];

// Only the SVG/DOM surface used by the renderer; no browser or network needed.
class Element {
  children = [];
  attributes = {};
  style = {};
  clientWidth = 800;
  clientHeight = 320;
  offsetHeight = 24;
  append(...nodes) { this.children.push(...nodes); }
  prepend(node) { this.children.unshift(node); }
  replaceChildren(...nodes) { this.children = nodes; }
  setAttribute(key, value) { this.attributes[key] = String(value); }
  querySelector() { return null; }
}
function render(metric, series) {
  const nodes = new Map(["data", "chart", "tooltip", "title", "subtitle", "metrics", "legend", "reset", "export"].map(id => [id, new Element()]));
  nodes.get("data").textContent = JSON.stringify({ title: "Probe", metrics: [{key: "loss", label: "Loss", ...metric}], series });
  const context = vm.createContext({
    document: {querySelector: selector => nodes.get(selector.slice(1)), createElement: () => new Element(), createElementNS: () => new Element()},
    ResizeObserver: class { observe() {} },
  });
  vm.runInContext(script + ";draw();globalThis.result=layout;", context);
  return { layout: context.result, svg: nodes.get("chart"), picker: nodes.get("metrics") };
}
const run = { name: "Arm", points: [{step: 1, loss: -5}, {step: 2, loss: -2}] };

test("negative-only bars and areas include zero and finite geometry", () => {
  for (const type of ["bar", "area"]) {
    const {layout, svg, picker} = render({type}, [run]);
    assert.ok(layout.ymax >= 0);
    assert.ok(layout.y(0) >= layout.top && layout.y(0) <= layout.height-layout.bottom);
    assert.equal(picker.hidden, true);
    for (const node of svg.children) {
      for (const value of Object.values(node.attributes)) assert.doesNotMatch(value, /NaN|Infinity/);
    }
    if (type === "bar") assert.ok(svg.children.filter(node => node.attributes.rx === "2").every(node => Number(node.attributes.height) > 0));
  }
});

test("scaling axes include fitted extrapolation on both axes", () => {
  const {layout} = render({type: "scaling"}, [{name: "Fit", points: [{step: 1, loss: 2}, {step: 10, loss: 3}], fit: [{step: 1, loss: 2}, {step: 50, loss: 8}]}]);
  assert.equal(layout.xmax, 50);
  assert.equal(layout.ymax, 8);
  assert.ok(Number.isFinite(layout.x(50)) && Number.isFinite(layout.y(8)));
});
