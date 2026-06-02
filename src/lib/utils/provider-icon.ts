/**
 * Monogram avatar (label + brand color) for provider cards.
 * Zero-asset: no icon library or bundled logos — scales to custom endpoints via a hash fallback.
 */

type IconSpec = { label: string; color: string };

/**
 * Brand-ish color + monogram per known provider id. CJK providers use their leading
 * character; latin ones use 1–2 letters. Foreground is derived for contrast — see readableFg.
 */
const BRAND: Record<string, IconSpec> = {
  anthropic: { label: "A", color: "#D97757" },
  deepseek: { label: "DS", color: "#4D6BFE" },
  kimi: { label: "Ki", color: "#5468FF" },
  "kimi-coding": { label: "Ki", color: "#5468FF" },
  zhipu: { label: "智", color: "#3859FF" },
  "zhipu-intl": { label: "Z", color: "#3859FF" },
  bailian: { label: "百", color: "#FF6A00" },
  "bailian-api": { label: "百", color: "#FF6A00" },
  doubao: { label: "豆", color: "#1664FF" },
  minimax: { label: "M", color: "#F23F5D" },
  "minimax-cn": { label: "M", color: "#F23F5D" },
  mimo: { label: "Mi", color: "#FF6900" },
  "mimo-tp": { label: "Mi", color: "#FF6900" },
  hunyuan: { label: "混", color: "#0052D9" },
  siliconflow: { label: "SF", color: "#7C3AED" },
  stepfun: { label: "阶", color: "#0A5CFF" },
  longcat: { label: "LC", color: "#FFC400" },
  iflytek: { label: "讯", color: "#1E50A2" },
  "tencent-coding": { label: "T", color: "#1AAD19" },
  openrouter: { label: "OR", color: "#6366F1" },
  aihubmix: { label: "AH", color: "#10B981" },
  zenmux: { label: "ZM", color: "#8B5CF6" },
  vercel: { label: "▲", color: "#3F3F46" },
  requesty: { label: "Rq", color: "#0EA5E9" },
  fireworks: { label: "Fw", color: "#FF6B35" },
  deepinfra: { label: "DI", color: "#0F766E" },
  novita: { label: "Nv", color: "#7C3AED" },
  ccswitch: { label: "CC", color: "#64748B" },
  ccr: { label: "CR", color: "#64748B" },
  ollama: { label: "Ol", color: "#52525B" },
};

const FALLBACK_PALETTE = [
  "#6366F1",
  "#0EA5E9",
  "#10B981",
  "#F59E0B",
  "#EF4444",
  "#8B5CF6",
  "#EC4899",
  "#14B8A6",
];

/** Derive a 1–2 char monogram from a display name (CJK → leading char; latin → initials). */
function deriveLabel(name: string): string {
  // Drop parentheticals like "(智谱 Intl)" so the monogram reflects the core brand
  const cleaned = name.replace(/[(（].*?[)）]/g, "").trim() || name;
  const chars = [...cleaned];
  const first = chars[0] ?? "?";
  if (/[㐀-鿿]/.test(first)) return first; // CJK ideograph
  const words = cleaned.split(/[\s\-_]+/).filter(Boolean);
  if (words.length >= 2 && words[0][0] && words[1][0]) {
    return (words[0][0] + words[1][0]).toUpperCase();
  }
  const latin = cleaned.replace(/[^a-zA-Z0-9]/g, "");
  return (latin.slice(0, 2) || first).toUpperCase();
}

/** Deterministic palette pick for unknown/custom ids (stable across reloads). */
function hashColor(s: string): string {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return FALLBACK_PALETTE[h % FALLBACK_PALETTE.length];
}

/** Pick black or white text for contrast against a hex background (perceived luminance). */
function readableFg(hex: string): string {
  const h = hex.replace("#", "");
  const full =
    h.length === 3
      ? h
          .split("")
          .map((c) => c + c)
          .join("")
      : h;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.6 ? "#1a1a1a" : "#ffffff";
}

/** Resolve a provider's avatar: monogram label, background color, readable foreground. */
export function providerIcon(id: string, name: string): { label: string; bg: string; fg: string } {
  const spec = BRAND[id] ?? { label: deriveLabel(name), color: hashColor(id) };
  return { label: spec.label, bg: spec.color, fg: readableFg(spec.color) };
}
