<script lang="ts">
  let {
    value = $bindable(""),
    placeholder = "",
    type = "text",
    disabled = false,
    class: className = "",
    oninput,
    onblur,
  }: {
    value?: string;
    placeholder?: string;
    type?: string;
    disabled?: boolean;
    class?: string;
    oninput?: (e: Event) => void;
    onblur?: (e: FocusEvent) => void;
  } = $props();
</script>

<input
  {type}
  {placeholder}
  {disabled}
  bind:value
  {oninput}
  {onblur}
  class="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 {className}"
/>
