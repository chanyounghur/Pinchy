import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";

const source = readFileSync(new URL("../src/selectionScroll.ts", import.meta.url), "utf8");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 },
});
const { createSelectionScroller } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`);
let now = 0;
let nextId = 1;
let reducedMotion = false;
const frames = new Map();
globalThis.requestAnimationFrame = (callback) => {
  const id = nextId++;
  frames.set(id, callback);
  return id;
};
globalThis.cancelAnimationFrame = (id) => frames.delete(id);
globalThis.window = { matchMedia: () => ({ matches: reducedMotion }) };
Object.defineProperty(globalThis, "performance", { value: { now: () => now }, configurable: true });
const advance = (ms = 16) => {
  now += ms;
  const pending = [...frames.values()];
  frames.clear();
  pending.forEach((callback) => callback(now));
};
const list = { scrollLeft: 0, scrollWidth: 10000, clientWidth: 1000 };
const scroller = createSelectionScroller(list);

// Repeated keys must keep the pending frame and make continuous forward progress.
scroller.to(234);
for (let i = 1; i <= 20; i++) {
  const pendingFrame = [...frames.keys()][0];
  scroller.to(i * 234);
  assert.equal([...frames.keys()][0], pendingFrame);
  const before = list.scrollLeft;
  advance(32);
  assert.ok(list.scrollLeft > before);
  assert.ok(list.scrollLeft <= i * 234);
  assert.equal(frames.size, 1);
}
// Releasing the key settles at the last destination with no animation left over.
for (let i = 0; i < 50; i++) advance();
assert.equal(list.scrollLeft, 4680);
assert.equal(frames.size, 0);

scroller.to(1000);
advance();
assert.ok(list.scrollLeft < 4680, "direction changes take effect on the next frame");
scroller.stop();
const stopped = list.scrollLeft;
advance();
assert.equal(list.scrollLeft, stopped, "manual scrolling can cancel the animation");

reducedMotion = true;
scroller.to(50000);
assert.equal(list.scrollLeft, 9000);
scroller.to(-100);
assert.equal(list.scrollLeft, 0);
assert.equal(frames.size, 0);

reducedMotion = false;
const afterDuration = (step) => {
  list.scrollLeft = 0;
  scroller.to(1000);
  for (let elapsed = 0; elapsed < 240; elapsed += step) advance(step);
  const position = list.scrollLeft;
  scroller.stop();
  return position;
};
assert.ok(Math.abs(afterDuration(8) - afterDuration(16)) < 0.01,
  "display refresh rate must not change the animation speed");
console.log("Selection scrolling: repeat, settling, reversal, cancellation, bounds, reduced motion and frame rates passed.");
