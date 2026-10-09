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
  getBoundingClientRect() { return {left: 0, top: 0}; }
}
function render(metric, series, interaction = "") {
  const nodes = new Map(["data", "chart", "tooltip", "title", "subtitle", "metrics", "legend", "reset", "export"].map(id => [id, new Element()]));
  nodes.get("data").textContent = JSON.stringify({ title: "Probe", metrics: [{key: "loss", label: "Loss", ...metric}], series });
  const context = vm.createContext({
    document: {querySelector: selector => nodes.get(selector.slice(1)), createElement: () => new Element(), createElementNS: () => new Element()},
    ResizeObserver: class { observe() {} },
  });
  vm.runInContext(script + ";draw();" + interaction + ";globalThis.result=layout;", context);
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

test("scaling zoom bounds use the fitted curve without connecting measured markers", () => {
  const {layout} = render({type: "scaling"}, [{name: "Fit", points: [{step: 1, loss: 100}, {step: 10, loss: 10}], fit: [{step: 1, loss: 1}, {step: 10, loss: 1}]}], "domain={x:[4,6]};draw()");
  assert.equal(layout.xmin, 4);
  assert.equal(layout.xmax, 6);
  assert.ok(layout.ymin < 1 && layout.ymax > 1 && layout.ymax < 2);
});

test("hover rings stay complete at endpoints but skip values outside a selected y-range", () => {
  const series = [{name: "Arm", points: [{step: 1, loss: 1.22}, {step: 10, loss: 1.05}]}];
  for (const step of [1, 10]) {
    const {svg} = render({type: "line"}, series, `domain={x:[1,10],y:[1,1.2]};draw();inspect(${step})`);
    const cursor = svg.children.find(node => node.attributes.id === "cursor");
    const circles = cursor.children.filter(node => node.attributes.r === "5");
    assert.equal(circles.length, step === 1 ? 0 : 1);
    assert.ok(circles.every(node => !node.attributes["clip-path"]));
  }
});


test("refining either zoom axis preserves the other selection", () => {
  for (const axis of ["x", "y"]) {
    const interaction = `domain={x:[2,8],y:[1.2,1.8]};draw();drag={x:layout.x(3),y:layout.y(1.6)};chart.onpointerup({clientX:layout.x(${axis === "x" ? 6 : 3}),clientY:layout.y(${axis === "y" ? 1.4 : 1.6})})`;
    const {layout} = render({type: "line"}, [{name: "Arm",points:[{step:1,loss:1},{step:10,loss:2}]}], interaction);
    if (axis === "x") assert.deepEqual([layout.ymin,layout.ymax],[1.2,1.8]);
    else assert.deepEqual([layout.xmin,layout.xmax],[2,8]);
  }
});

test("small fractional x ticks retain distinct values on linear and log axes", () => {
  for (const xScale of ["linear", "log"]) {
    const {svg,layout} = render({type:"scaling",xScale,xLabel:"Scale"}, [{name:"Arm",points:[{step:.001,loss:1},{step:.01,loss:2}]}]);
    const ticks = svg.children.filter(node=>node.attributes.y===String(layout.height-layout.bottom+23)).map(node=>Number(node.textContent));
    assert.equal(ticks.length,5);
    assert.equal(new Set(ticks).size,5);
    assert.ok(ticks.every(tick=>tick>0));
  }
});


test("narrow ranges at large x-values keep fractional ticks distinct", () => {
  for (const xScale of ["linear", "log"]) {
    const {svg,layout} = render({type:"line",xScale,xLabel:"Scale"}, [{name:"Arm",points:[{step:100000.1,loss:1},{step:100000.9,loss:2}]}]);
    const ticks=svg.children.filter(node=>node.attributes.y===String(layout.height-layout.bottom+23)).map(node=>Number(node.textContent));
    assert.equal(new Set(ticks).size,5);
    assert.ok(ticks.every((tick,i)=>Math.abs(tick-(100000.1+i*.2))<.01));
  }
});


test("a tight training-step zoom keeps its single integer tick exact", () => {
  const {svg,layout}=render({type:"line"},[{name:"Arm",points:[{step:123,loss:1},{step:125,loss:2}]}],"domain={x:[123.2,124.8]};draw()");
  const ticks=svg.children.filter(node=>node.attributes.y===String(layout.height-layout.bottom+23)).map(node=>Number(node.textContent));
  assert.deepEqual(ticks,[124]);
});


test("y-only zoom retains confidence bands crossing the selected range", () => {
  const {layout}=render({type:"line"},[
    {name:"Band",points:[{step:1,loss:10,lower:0,upper:20},{step:10,loss:10,lower:0,upper:20}]},
    {name:"Other",points:[{step:20,loss:1},{step:30,loss:1}]},
  ],"domain={y:[.5,1.5]};draw()");
  assert.deepEqual([layout.xmin,layout.xmax],[1,30]);
  const crossing=render({type:"line"},[{name:"Band",points:[{step:1,loss:0,lower:-1,upper:1},{step:10,loss:10,lower:9,upper:11}]}],"domain={y:[4,6]};draw()");
  assert.ok(crossing.layout.xmin<4.6 && crossing.layout.xmax>6.4);
});

test("arrow inspection skips fitted-only slices without creating invalid cursors", () => {
  const {svg}=render({type:"scaling"},[{name:"Fit",points:[{step:1,loss:2},{step:10,loss:2}],fit:[{step:1,loss:1},{step:10,loss:1}]}],"domain={x:[4,6]};draw();chart.onkeydown({key:'ArrowRight',preventDefault(){}})");
  assert.ok(!svg.children.some(node=>node.attributes.id==='cursor'));
});
