/**
 * Inline paste placeholder tokens.
 *
 * Long pastes are kept out of the textarea body as chips, but we drop a position-anchored
 * placeholder token at the cursor so the block expands back at the RIGHT spot on send
 * (issue #156). The token is keyed on the block's display sequence (`#N`), which is unique
 * among the current blocks and language-independent — so parsing survives label localization,
 * and the user never sees a machine id.
 *
 * Token shape: `[<label>]` where the label contains `#N · ` e.g. `[Pasted #1 · 120 lines]`.
 * The `#N · ` (number + middle-dot separator) is the parse anchor.
 */

/** Match a token and capture its `#N` sequence. Forbids nested brackets so `[...]` stays balanced. */
const TOKEN_RE = /\[[^[\]]*#(\d+) · [^[\]]*\]/g;

/** Build a token from an already-localized label (which must contain `#N · `). */
export function buildPasteToken(label: string): string {
  return `[${label}]`;
}

/** Regex matching exactly the token for one sequence. `#1 · ` can't match `#10 · ` (digit vs space). */
export function singlePasteTokenRe(seq: number): RegExp {
  return new RegExp(`\\[[^[\\]]*#${seq} · [^[\\]]*\\]`);
}

/** Insert/replace at the selection range (selStart..selEnd). Empty range = plain insert. */
export function insertAtSelection(
  text: string,
  selStart: number,
  selEnd: number,
  insert: string,
): string {
  return text.slice(0, selStart) + insert + text.slice(selEnd);
}

/** Block sequences referenced by tokens in `body`, in document order (duplicates kept). */
export function parsePasteTokenSeqs(body: string): number[] {
  const seqs: number[] = [];
  const re = new RegExp(TOKEN_RE.source, "g");
  let m: RegExpExecArray | null;
  while ((m = re.exec(body)) !== null) seqs.push(Number(m[1]));
  return seqs;
}

/**
 * Expand every paste token in `body` to its block's full text, in place.
 * Returns the expanded text plus the set of block sequences actually consumed
 * (so the caller can append any orphan blocks that had no token).
 * A token whose sequence has no matching block is stripped (defensive — shouldn't occur).
 */
export function expandPasteTokens(
  body: string,
  blocks: ReadonlyArray<{ seq?: number; text: string }>,
): { text: string; usedSeqs: Set<number> } {
  const usedSeqs = new Set<number>();
  const re = new RegExp(TOKEN_RE.source, "g");
  const text = body.replace(re, (_m, seqStr: string) => {
    const seq = Number(seqStr);
    const blk = blocks.find((b) => b.seq === seq);
    if (!blk) return "";
    usedSeqs.add(seq);
    return blk.text;
  });
  return { text, usedSeqs };
}
