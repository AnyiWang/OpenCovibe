/**
 * Yield to the browser's main thread before continuing.
 *
 * Prefers `requestAnimationFrame` so the browser can paint and process input
 * between chunks. Falls back to a 50ms `setTimeout` in case `rAF` is throttled
 * (backgrounded WebView, hidden tab) — first to fire wins, the other is cleared.
 */
export function yieldToMain(): Promise<void> {
  if (typeof requestAnimationFrame !== "function") {
    return new Promise((r) => setTimeout(r, 0));
  }
  return new Promise<void>((resolve) => {
    let done = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const finish = () => {
      if (done) return;
      done = true;
      if (timer !== null) clearTimeout(timer);
      resolve();
    };
    requestAnimationFrame(finish);
    timer = setTimeout(finish, 50);
  });
}
