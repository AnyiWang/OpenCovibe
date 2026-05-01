import { dbg } from "./debug";

/**
 * Runtime perf-mode check. Read on every call so toggling localStorage doesn't require a refresh.
 * localStorage hits short-circuit; URL fallback uses cheap string includes.
 *
 * Cost when disabled: one localStorage.getItem (~1µs). Insertion points are sparse, negligible.
 *
 * Exported for ad-hoc gates (e.g., manual `performance.now()` blocks that don't fit perfMark).
 */
export function isPerfEnabled(): boolean {
  if (typeof window === "undefined") return false;
  if (localStorage.getItem("ocv:debug")) return true;
  return window.location.search.includes("debug");
}

/** Wrap a synchronous fn; logs duration via `dbg("perf", label, ...)` when enabled and >1ms. */
export function perfMark<T>(label: string, fn: () => T, meta?: Record<string, unknown>): T {
  if (!isPerfEnabled()) return fn();
  const t0 = performance.now();
  const result = fn();
  const dt = performance.now() - t0;
  if (dt > 1) dbg("perf", label, { ms: +dt.toFixed(2), ...(meta ?? {}) });
  return result;
}

/** Async variant. Same gating as perfMark. */
export async function perfMarkAsync<T>(
  label: string,
  fn: () => Promise<T>,
  meta?: Record<string, unknown>,
): Promise<T> {
  if (!isPerfEnabled()) return fn();
  const t0 = performance.now();
  const result = await fn();
  const dt = performance.now() - t0;
  if (dt > 1) dbg("perf", label, { ms: +dt.toFixed(2), ...(meta ?? {}) });
  return result;
}

/**
 * rAF-based fps counter. Returns a stop fn that emits average fps over the measurement window.
 * Use for diagnosing drag/scroll smoothness.
 *
 * Example:
 *   const stop = fpsCounter("aside-drag");
 *   // ... user dragging ...
 *   stop();  // emits "[ocv:perf] aside-drag fps { fps: 58.3, frames: 233, ms: 4000 }"
 */
export function fpsCounter(label: string): () => void {
  if (!isPerfEnabled()) return () => {};
  let frames = 0;
  let rafId = 0;
  const t0 = performance.now();
  const tick = () => {
    frames++;
    rafId = requestAnimationFrame(tick);
  };
  rafId = requestAnimationFrame(tick);
  return () => {
    cancelAnimationFrame(rafId);
    const dt = performance.now() - t0;
    dbg("perf", `${label} fps`, {
      fps: +((frames / dt) * 1000).toFixed(1),
      frames,
      ms: +dt.toFixed(0),
    });
  };
}
