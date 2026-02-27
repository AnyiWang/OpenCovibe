/**
 * Lightweight ANSI escape code → HTML converter.
 * Handles SGR (Select Graphic Rendition) codes used by CLI tools like Claude Code.
 * Supports: 8 standard colors, bright variants, bold, dim, italic, underline, reset.
 */

// Standard ANSI color names → CSS classes (we use Tailwind-compatible names)
const FG_COLORS: Record<number, string> = {
  30: "#6b7280", // black (gray-500 for visibility on dark bg)
  31: "#ef4444", // red
  32: "#22c55e", // green
  33: "#eab308", // yellow
  34: "#3b82f6", // blue
  35: "#a855f7", // magenta
  36: "#06b6d4", // cyan
  37: "#d1d5db", // white (gray-300)
  // Bright variants
  90: "#9ca3af", // bright black (gray-400)
  91: "#f87171", // bright red
  92: "#4ade80", // bright green
  93: "#facc15", // bright yellow
  94: "#60a5fa", // bright blue
  95: "#c084fc", // bright magenta
  96: "#22d3ee", // bright cyan
  97: "#f3f4f6", // bright white
};

const BG_COLORS: Record<number, string> = {
  40: "#374151",
  41: "#991b1b",
  42: "#166534",
  43: "#854d0e",
  44: "#1e3a5f",
  45: "#6b21a8",
  46: "#155e75",
  47: "#e5e7eb",
  100: "#4b5563",
  101: "#b91c1c",
  102: "#15803d",
  103: "#a16207",
  104: "#1d4ed8",
  105: "#7e22ce",
  106: "#0891b2",
  107: "#f9fafb",
};

interface Style {
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

function styleToAttrs(s: Style): string {
  const parts: string[] = [];
  if (s.fg) parts.push(`color:${s.fg}`);
  if (s.bg) parts.push(`background-color:${s.bg}`);
  if (s.bold) parts.push("font-weight:bold");
  if (s.dim) parts.push("opacity:0.6");
  if (s.italic) parts.push("font-style:italic");
  if (s.underline) parts.push("text-decoration:underline");
  return parts.length > 0 ? ` style="${parts.join(";")}"` : "";
}

function hasStyle(s: Style): boolean {
  return !!(s.fg || s.bg || s.bold || s.dim || s.italic || s.underline);
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Convert a string containing ANSI escape codes to HTML.
 * Returns sanitized HTML safe for {@html} rendering.
 */
export function ansiToHtml(input: string): string {
  // Match ANSI CSI sequences: ESC[ ... m
  // eslint-disable-next-line no-control-regex
  const ansiRegex = /\x1b\[([0-9;]*)m/g;
  const style: Style = {};
  let result = "";
  let lastIndex = 0;
  let spanOpen = false;

  let match;
  while ((match = ansiRegex.exec(input)) !== null) {
    // Append text before this escape sequence
    const textBefore = input.slice(lastIndex, match.index);
    if (textBefore) {
      result += escapeHtml(textBefore);
    }
    lastIndex = match.index + match[0].length;

    // Parse SGR codes
    const codes = match[1] ? match[1].split(";").map(Number) : [0];
    for (let i = 0; i < codes.length; i++) {
      const code = codes[i];
      if (code === 0) {
        // Reset all
        delete style.fg;
        delete style.bg;
        delete style.bold;
        delete style.dim;
        delete style.italic;
        delete style.underline;
      } else if (code === 1) {
        style.bold = true;
      } else if (code === 2) {
        style.dim = true;
      } else if (code === 3) {
        style.italic = true;
      } else if (code === 4) {
        style.underline = true;
      } else if (code === 22) {
        delete style.bold;
        delete style.dim;
      } else if (code === 23) {
        delete style.italic;
      } else if (code === 24) {
        delete style.underline;
      } else if (code === 39) {
        delete style.fg;
      } else if (code === 49) {
        delete style.bg;
      } else if (FG_COLORS[code]) {
        style.fg = FG_COLORS[code];
      } else if (BG_COLORS[code]) {
        style.bg = BG_COLORS[code];
      } else if (code === 38 && codes[i + 1] === 5) {
        // 256-color foreground: \e[38;5;Nm — map to approximate hex
        style.fg = color256ToHex(codes[i + 2] ?? 0);
        i += 2;
      } else if (code === 48 && codes[i + 1] === 5) {
        // 256-color background
        style.bg = color256ToHex(codes[i + 2] ?? 0);
        i += 2;
      }
    }

    // Close previous span if open
    if (spanOpen) {
      result += "</span>";
      spanOpen = false;
    }

    // Open new span if style is active
    if (hasStyle(style)) {
      result += `<span${styleToAttrs(style)}>`;
      spanOpen = true;
    }
  }

  // Append remaining text after last escape sequence
  const remaining = input.slice(lastIndex);
  if (remaining) {
    result += escapeHtml(remaining);
  }

  // Close any open span
  if (spanOpen) {
    result += "</span>";
  }

  // Strip any remaining non-SGR escape sequences (cursor movement, etc.)
  // eslint-disable-next-line no-control-regex
  return result.replace(/\x1b\[[0-9;]*[A-HJKSTfhilmnsu]/g, "");
}

/** Map 256-color index to hex. */
function color256ToHex(n: number): string {
  if (n < 16) {
    // Standard 16 colors
    const palette = [
      "#000000",
      "#aa0000",
      "#00aa00",
      "#aa5500",
      "#0000aa",
      "#aa00aa",
      "#00aaaa",
      "#aaaaaa",
      "#555555",
      "#ff5555",
      "#55ff55",
      "#ffff55",
      "#5555ff",
      "#ff55ff",
      "#55ffff",
      "#ffffff",
    ];
    return palette[n] ?? "#aaaaaa";
  }
  if (n < 232) {
    // 216 color cube: 6×6×6
    const idx = n - 16;
    const r = Math.floor(idx / 36);
    const g = Math.floor((idx % 36) / 6);
    const b = idx % 6;
    const toHex = (v: number) => (v === 0 ? 0 : 55 + v * 40).toString(16).padStart(2, "0");
    return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
  }
  // Grayscale: 24 shades
  const level = 8 + (n - 232) * 10;
  const hex = level.toString(16).padStart(2, "0");
  return `#${hex}${hex}${hex}`;
}

/**
 * Check if a string contains ANSI escape sequences.
 */
export function hasAnsiCodes(text: string): boolean {
  // eslint-disable-next-line no-control-regex
  return /\x1b\[/.test(text);
}
