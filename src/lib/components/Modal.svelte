<script lang="ts">
  let {
    open = $bindable(false),
    title = "",
    children,
  }: {
    open?: boolean;
    title?: string;
    children?: import("svelte").Snippet;
  } = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      open = false;
    }
  }

  function handleBackdropClick() {
    open = false;
  }
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onkeydown={handleKeydown}
  >
    <!-- Backdrop -->
    <div
      class="fixed inset-0 bg-black/60 backdrop-blur-sm"
      onclick={handleBackdropClick}
      role="presentation"
    ></div>

    <!-- Content -->
    <div class="relative z-50 w-full max-w-lg rounded-lg border bg-background p-6 shadow-lg">
      {#if title}
        <h2 class="mb-4 text-lg font-semibold">{title}</h2>
      {/if}
      {#if children}
        {@render children()}
      {/if}
    </div>
  </div>
{/if}
