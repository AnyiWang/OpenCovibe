<script lang="ts">
  import { imageViewer } from "$lib/stores/image-viewer.svelte";
  import { t } from "$lib/i18n/index.svelte";

  let dialogEl: HTMLDivElement | undefined = $state();
  let imgEl: HTMLImageElement | undefined = $state();

  // Zoom/pan state. scale === 1 means "fit" (the <img> is object-contain); >1 zooms in.
  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  // `panning` drives the cursor + transition in the template, so it must be reactive.
  let panning = $state(false);
  // Non-reactive drag bookkeeping (mutated inside pointer handlers, never rendered).
  let didPan = false;
  let lastX = 0;
  let lastY = 0;

  let copyState = $state<"idle" | "copied" | "failed">("idle");
  let copyTimer: ReturnType<typeof setTimeout> | null = null;

  const MIN = 1;
  const MAX = 8;
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

  // Focus the overlay on open (so Escape / arrows land here) and reset zoom whenever the
  // viewer opens or pages to a different image. Reading imageViewer.index tracks paging.
  $effect(() => {
    if (imageViewer.open) {
      void imageViewer.index;
      resetZoom();
      dialogEl?.focus();
    }
  });

  function resetZoom() {
    scale = MIN;
    tx = 0;
    ty = 0;
  }

  /** Zoom to `newScale` keeping the point under (clientX, clientY) fixed on screen. */
  function focalZoom(newScale: number, clientX: number, clientY: number) {
    const rect = dialogEl?.getBoundingClientRect();
    if (!rect) return;
    const ns = clamp(newScale, MIN, MAX);
    if (ns === MIN) {
      resetZoom();
      return;
    }
    // Cursor offset from the (untransformed) image center ≈ overlay center.
    const dx = clientX - (rect.left + rect.width / 2);
    const dy = clientY - (rect.top + rect.height / 2);
    const ratio = ns / scale;
    tx = dx * (1 - ratio) + ratio * tx;
    ty = dy * (1 - ratio) + ratio * ty;
    scale = ns;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
    focalZoom(scale * factor, e.clientX, e.clientY);
  }

  /** Scale that renders the image at its native pixel size (for the click-to-100% toggle). */
  function actualPixelScale(): number {
    if (!imgEl || !imgEl.clientWidth) return 2;
    const s = imgEl.naturalWidth / imgEl.clientWidth;
    return s > 1.05 ? clamp(s, MIN, MAX) : 2;
  }

  function onImgClick(e: MouseEvent) {
    // A pan drag ends with a click event — swallow it so it doesn't also toggle zoom.
    if (didPan) {
      didPan = false;
      return;
    }
    if (scale > MIN) resetZoom();
    else focalZoom(actualPixelScale(), e.clientX, e.clientY);
  }

  function onPointerDown(e: PointerEvent) {
    if (scale <= MIN) return; // only pan when zoomed in
    panning = true;
    didPan = false;
    lastX = e.clientX;
    lastY = e.clientY;
    imgEl?.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!panning) return;
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    if (Math.abs(dx) + Math.abs(dy) > 3) didPan = true;
    tx += dx;
    ty += dy;
    lastX = e.clientX;
    lastY = e.clientY;
  }

  function onPointerUp(e: PointerEvent) {
    if (!panning) return;
    panning = false;
    imgEl?.releasePointerCapture?.(e.pointerId);
  }

  async function copyImage() {
    const el = imgEl;
    if (!el) return;
    if (copyTimer) clearTimeout(copyTimer);
    try {
      if (!navigator.clipboard || !window.isSecureContext || typeof ClipboardItem === "undefined") {
        throw new Error("clipboard-unavailable");
      }
      // Draw the already-loaded <img> to a canvas → re-encodes any format to PNG (the only
      // format the clipboard API reliably accepts). A cross-origin image taints the canvas
      // and toBlob() throws — caught below and surfaced as "Copy failed".
      const canvas = document.createElement("canvas");
      canvas.width = el.naturalWidth;
      canvas.height = el.naturalHeight;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("no-2d-context");
      ctx.drawImage(el, 0, 0);
      const blob = await new Promise<Blob>((res, rej) =>
        canvas.toBlob((b) => (b ? res(b) : rej(new Error("toBlob-null"))), "image/png"),
      );
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      copyState = "copied";
    } catch {
      copyState = "failed";
    }
    copyTimer = setTimeout(() => (copyState = "idle"), 1600);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      imageViewer.close();
    } else if (e.key === "ArrowLeft" && imageViewer.images.length > 1) {
      e.preventDefault();
      imageViewer.prev();
    } else if (e.key === "ArrowRight" && imageViewer.images.length > 1) {
      e.preventDefault();
      imageViewer.next();
    } else if ((e.key === "c" || e.key === "C") && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      copyImage();
    }
  }
</script>

{#if imageViewer.open && imageViewer.current}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-[60] select-none"
    role="dialog"
    aria-modal="true"
    aria-label={imageViewer.current.name ?? "Image viewer"}
    tabindex="-1"
    bind:this={dialogEl}
    onkeydown={onKeydown}
  >
    <!-- Backdrop: click anywhere outside the image to close -->
    <button
      type="button"
      class="absolute inset-0 h-full w-full cursor-default bg-black/80 backdrop-blur-sm"
      aria-label={t("imageViewer_close")}
      onclick={() => imageViewer.close()}
    ></button>

    <!-- Toolbar -->
    <div class="absolute right-3 top-3 z-10 flex items-center gap-1.5">
      <button
        type="button"
        onclick={copyImage}
        title={t("imageViewer_copy")}
        class="flex items-center gap-1.5 rounded-md border border-white/20 bg-white/10 px-2.5 py-1.5 text-xs font-medium text-white backdrop-blur-sm transition-colors hover:bg-white/20"
      >
        {#if copyState === "copied"}
          <svg
            class="h-3.5 w-3.5 text-emerald-400"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
          >
          {t("imageViewer_copied")}
        {:else if copyState === "failed"}
          <svg
            class="h-3.5 w-3.5 text-red-400"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"><path d="M18 6 6 18M6 6l12 12" /></svg
          >
          {t("imageViewer_copyFailed")}
        {:else}
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            ><rect width="14" height="14" x="8" y="8" rx="2" /><path
              d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"
            /></svg
          >
          {t("imageViewer_copy")}
        {/if}
      </button>
      <button
        type="button"
        onclick={() => imageViewer.close()}
        aria-label={t("imageViewer_close")}
        class="rounded-md border border-white/20 bg-white/10 p-1.5 text-white backdrop-blur-sm transition-colors hover:bg-white/20"
      >
        <svg
          class="h-4 w-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="M18 6 6 18M6 6l12 12" /></svg
        >
      </button>
    </div>

    <!-- Paging (only for multi-image messages) -->
    {#if imageViewer.images.length > 1}
      <button
        type="button"
        onclick={() => imageViewer.prev()}
        aria-label={t("imageViewer_prev")}
        class="absolute left-3 top-1/2 z-10 -translate-y-1/2 rounded-full border border-white/20 bg-white/10 p-2 text-white backdrop-blur-sm transition-colors hover:bg-white/20"
      >
        <svg
          class="h-5 w-5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="m15 18-6-6 6-6" /></svg
        >
      </button>
      <button
        type="button"
        onclick={() => imageViewer.next()}
        aria-label={t("imageViewer_next")}
        class="absolute right-3 top-1/2 z-10 -translate-y-1/2 rounded-full border border-white/20 bg-white/10 p-2 text-white backdrop-blur-sm transition-colors hover:bg-white/20"
      >
        <svg
          class="h-5 w-5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
        >
      </button>
      <div
        class="absolute bottom-3 left-1/2 z-10 -translate-x-1/2 rounded-full bg-black/50 px-2.5 py-1 text-xs text-white/90 backdrop-blur-sm"
      >
        {imageViewer.index + 1} / {imageViewer.images.length}
      </div>
    {/if}

    <!-- Image layer: container ignores pointer events so clicks in the margin fall through to
         the backdrop (close); only the <img> itself is interactive (zoom/pan). -->
    <div class="pointer-events-none absolute inset-0 flex items-center justify-center p-6 sm:p-10">
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
      <img
        bind:this={imgEl}
        src={imageViewer.current.src}
        alt={imageViewer.current.name ?? ""}
        draggable="false"
        onwheel={onWheel}
        onclick={onImgClick}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        onpointercancel={onPointerUp}
        class="pointer-events-auto max-h-full max-w-full object-contain shadow-2xl {scale > MIN
          ? panning
            ? 'cursor-grabbing'
            : 'cursor-grab'
          : 'cursor-zoom-in'}"
        style="transform: translate({tx}px, {ty}px) scale({scale}); transition: {panning
          ? 'none'
          : 'transform 60ms linear'};"
      />
    </div>
  </div>
{/if}
