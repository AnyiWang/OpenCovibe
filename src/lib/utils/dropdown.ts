// Shared dropdown positioning + dismissal helpers.
// Both auth badges (and other anchored dropdowns) flip above the trigger when there
// isn't enough room below, and close on outside-click / Escape. These helpers keep
// that logic in one place instead of copy-pasting it into each component.

/**
 * Compute a `position:fixed` inline style for a dropdown anchored to `buttonEl`.
 * Flips above the trigger when the space below is smaller than `minSpaceBelow`.
 */
export function computeDropdownStyle(buttonEl: HTMLElement, minSpaceBelow: number): string {
  const rect = buttonEl.getBoundingClientRect();
  const spaceBelow = window.innerHeight - rect.bottom;
  if (spaceBelow < minSpaceBelow) {
    return `position:fixed; bottom:${window.innerHeight - rect.top + 4}px; left:${rect.left}px; z-index:50;`;
  }
  return `position:fixed; top:${rect.bottom + 4}px; left:${rect.left}px; z-index:50;`;
}

/**
 * Wire outside-click (capture-phase mousedown) and Escape to dismiss a dropdown.
 * `isOpen` is read lazily so the handlers see the current open state; `close` runs
 * when a dismissal gesture fires while open. Returns a teardown function — call it
 * from the component's `onMount` cleanup.
 */
export function attachDismissHandlers(opts: {
  getWrapper: () => HTMLElement | undefined;
  isOpen: () => boolean;
  close: () => void;
}): () => void {
  function onDocClick(e: MouseEvent) {
    const wrapper = opts.getWrapper();
    if (opts.isOpen() && wrapper && !wrapper.contains(e.target as Node)) opts.close();
  }
  function onDocKeydown(e: KeyboardEvent) {
    if (opts.isOpen() && e.key === "Escape") opts.close();
  }
  document.addEventListener("mousedown", onDocClick, true);
  document.addEventListener("keydown", onDocKeydown);
  return () => {
    document.removeEventListener("mousedown", onDocClick, true);
    document.removeEventListener("keydown", onDocKeydown);
  };
}
