<script lang="ts">
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n/index.svelte";

  export type CodexReviewKind = "uncommitted" | "base" | "commit" | "custom";

  let {
    open = $bindable(false),
    onSubmit,
  }: {
    open?: boolean;
    onSubmit?: (choice: { kind: CodexReviewKind; value: string }) => void;
  } = $props();

  let kind = $state<CodexReviewKind>("uncommitted");
  let branch = $state("main");
  let commit = $state("");
  let custom = $state("");

  // The value required for the selected kind (empty kinds need no input).
  let requiredValue = $derived(
    kind === "base"
      ? branch.trim()
      : kind === "commit"
        ? commit.trim()
        : kind === "custom"
          ? custom.trim()
          : "x",
  );
  let canRun = $derived(requiredValue.length > 0);

  const OPTIONS: { kind: CodexReviewKind; label: () => string; desc: () => string }[] = [
    {
      kind: "uncommitted",
      label: () => t("codexReview_optUncommitted"),
      desc: () => t("codexReview_optUncommittedDesc"),
    },
    {
      kind: "base",
      label: () => t("codexReview_optBase"),
      desc: () => t("codexReview_optBaseDesc"),
    },
    {
      kind: "commit",
      label: () => t("codexReview_optCommit"),
      desc: () => t("codexReview_optCommitDesc"),
    },
    {
      kind: "custom",
      label: () => t("codexReview_optCustom"),
      desc: () => t("codexReview_optCustomDesc"),
    },
  ];

  function run() {
    if (!canRun) return;
    const value =
      kind === "base"
        ? branch.trim()
        : kind === "commit"
          ? commit.trim()
          : kind === "custom"
            ? custom.trim()
            : "";
    onSubmit?.({ kind, value });
    open = false;
  }
</script>

<Modal bind:open title={t("codexReview_pickerTitle")}>
  <div class="space-y-3 min-w-[420px] max-w-[520px]">
    {#each OPTIONS as opt (opt.kind)}
      <label
        class="flex items-start gap-3 rounded-md border p-3 cursor-pointer transition-colors
          {kind === opt.kind ? 'border-primary bg-primary/5' : 'hover:bg-accent'}"
      >
        <input
          type="radio"
          name="codex-review-kind"
          value={opt.kind}
          bind:group={kind}
          class="mt-0.5"
        />
        <div class="flex-1 min-w-0">
          <p class="text-sm font-medium">{opt.label()}</p>
          <p class="text-xs text-muted-foreground mt-0.5">{opt.desc()}</p>
        </div>
      </label>
    {/each}

    {#if kind === "base"}
      <input
        class="w-full rounded-md border px-3 py-2 text-sm bg-background"
        placeholder={t("codexReview_branchPlaceholder")}
        bind:value={branch}
      />
    {:else if kind === "commit"}
      <input
        class="w-full rounded-md border px-3 py-2 text-sm bg-background font-mono"
        placeholder={t("codexReview_commitPlaceholder")}
        bind:value={commit}
      />
    {:else if kind === "custom"}
      <textarea
        class="w-full rounded-md border px-3 py-2 text-sm bg-background min-h-[80px]"
        placeholder={t("codexReview_customPlaceholder")}
        bind:value={custom}
      ></textarea>
    {/if}

    <div class="flex justify-end gap-2 pt-1">
      <button
        class="rounded-md border px-3 py-1.5 text-sm hover:bg-accent"
        onclick={() => (open = false)}
      >
        {t("common_cancel")}
      </button>
      <button
        class="rounded-md px-3 py-1.5 text-sm bg-primary text-primary-foreground disabled:opacity-50"
        disabled={!canRun}
        onclick={run}
      >
        {t("codexReview_run")}
      </button>
    </div>
  </div>
</Modal>
