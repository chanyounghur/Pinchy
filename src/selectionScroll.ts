// Keep one animation alive while key repeat updates its destination.
export function createSelectionScroller(list: HTMLElement) {
  let frame = 0;
  let target = list.scrollLeft;
  let position = target;
  let lastTime = 0;

  const stop = () => {
    cancelAnimationFrame(frame);
    frame = 0;
  };

  const tick = (time: number) => {
    const elapsed = Math.min(Math.max(time - lastTime, 0), 64);
    lastTime = time;
    target = Math.max(0, Math.min(target, list.scrollWidth - list.clientWidth));
    // Time-based easing keeps the same feel at different display refresh rates.
    position += (target - position) * (1 - Math.exp(-elapsed / 60));
    if (Math.abs(target - position) < 0.5) {
      list.scrollLeft = target;
      frame = 0;
      return;
    }
    list.scrollLeft = position;
    frame = requestAnimationFrame(tick);
  };

  return {
    stop,
    to(left: number) {
      target = Math.max(0, Math.min(left, list.scrollWidth - list.clientWidth));
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
        stop();
        list.scrollLeft = target;
      } else if (!frame) {
        position = list.scrollLeft;
        lastTime = performance.now();
        frame = requestAnimationFrame(tick);
      }
    },
  };
}
