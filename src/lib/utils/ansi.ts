/**
 * Lightweight ANSI escape code → HTML converter.
 * Handles SGR (Select Graphic Rendition) codes used by CLI tools like Claude Code.
 * Supports: 8 standard colors, bright variants, bold, dim, italic, underline, reset.
 */

// Standard ANSI colors use semantic classes so light, dark, and high-contrast themes can provide
// readable palettes. Extended 256-color values remain inline because they are output-defined.
const FG_COLORS: Record<number, string> = {
  30: "ansi-fg-black",
  31: "ansi-fg-red",
  32: "ansi-fg-green",
  33: "ansi-fg-yellow",
  34: "ansi-fg-blue",
  35: "ansi-fg-magenta",
  36: "ansi-fg-cyan",
  37: "ansi-fg-white",
  90: "ansi-fg-bright-black",
  91: "ansi-fg-bright-red",
  92: "ansi-fg-bright-green",
  93: "ansi-fg-bright-yellow",
  94: "ansi-fg-bright-blue",
  95: "ansi-fg-bright-magenta",
  96: "ansi-fg-bright-cyan",
  97: "ansi-fg-bright-white",
};

const BG_COLORS: Record<number, string> = {
  40: "ansi-bg-black",
  41: "ansi-bg-red",
  42: "ansi-bg-green",
  43: "ansi-bg-yellow",
  44: "ansi-bg-blue",
  45: "ansi-bg-magenta",
  46: "ansi-bg-cyan",
  47: "ansi-bg-white",
  100: "ansi-bg-bright-black",
  101: "ansi-bg-bright-red",
  102: "ansi-bg-bright-green",
  103: "ansi-bg-bright-yellow",
  104: "ansi-bg-bright-blue",
  105: "ansi-bg-bright-magenta",
  106: "ansi-bg-bright-cyan",
  107: "ansi-bg-bright-white",
};

interface Style {
  fgClass?: string;
  bgClass?: string;
  fgColor?: string;
  bgColor?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

function styleToAttrs(s: Style): string {
  const classes = [s.fgClass, s.bgClass].filter(Boolean);
  const parts: string[] = [];
  if (s.fgColor) parts.push(`color:${s.fgColor}`);
  if (s.bgColor) parts.push(`background-color:${s.bgColor}`);
  if (s.bold) parts.push("font-weight:bold");
  if (s.dim) parts.push("opacity:0.6");
  if (s.italic) parts.push("font-style:italic");
  if (s.underline) parts.push("text-decoration:underline");
  const classAttr = classes.length > 0 ? ` class="${classes.join(" ")}"` : "";
  const styleAttr = parts.length > 0 ? ` style="${parts.join(";")}"` : "";
  return `${classAttr}${styleAttr}`;
}

function hasStyle(s: Style): boolean {
  return !!(
    s.fgClass ||
    s.bgClass ||
    s.fgColor ||
    s.bgColor ||
    s.bold ||
    s.dim ||
    s.italic ||
    s.underline
  );
}

function clearForeground(style: Style): void {
  delete style.fgClass;
  delete style.fgColor;
}

function clearBackground(style: Style): void {
  delete style.bgClass;
  delete style.bgColor;
}

/**
 * Comprehensive ANSI escape sequence regex (4 alternations):
 * 1. CSI: \x1b[ + parameter bytes (0x30-0x3f, incl ? ; digits) + intermediate (0x20-0x2f) + final (0x40-0x7e)
 * 2. OSC: \x1b] + ... + ST (\x07 or \x1b\\)
 * 3. Charset designation: \x1b + intermediate (0x20-0x2f) + final (0x30-0x7e)
 * 4. Fe sequences: \x1b + byte in 0x40-0x5f
 */

/* eslint-disable no-control-regex */
const ANSI_RE =
  /\x1b(?:\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[\x20-\x2f][\x30-\x7e]|[\x40-\x5f])/g;
/* eslint-enable no-control-regex */

/** Strip all ANSI escape sequences, returning clean plain text. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI_RE, "");
}

export function escapeHtml(str: string): string {
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
        clearForeground(style);
        clearBackground(style);
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
        clearForeground(style);
      } else if (code === 49) {
        clearBackground(style);
      } else if (FG_COLORS[code]) {
        clearForeground(style);
        style.fgClass = FG_COLORS[code];
      } else if (BG_COLORS[code]) {
        clearBackground(style);
        style.bgClass = BG_COLORS[code];
      } else if (code === 38 && codes[i + 1] === 5) {
        // 256-color foreground: indices 0-15 share the theme-aware standard palette.
        clearForeground(style);
        const colorIndex = codes[i + 2] ?? 0;
        const standardCode = colorIndex < 8 ? 30 + colorIndex : 90 + colorIndex - 8;
        if (colorIndex < 16) style.fgClass = FG_COLORS[standardCode];
        else style.fgColor = color256ToHex(colorIndex);
        i += 2;
      } else if (code === 48 && codes[i + 1] === 5) {
        // 256-color background
        clearBackground(style);
        const colorIndex = codes[i + 2] ?? 0;
        const standardCode = colorIndex < 8 ? 40 + colorIndex : 100 + colorIndex - 8;
        if (colorIndex < 16) style.bgClass = BG_COLORS[standardCode];
        else style.bgColor = color256ToHex(colorIndex);
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

  // Strip any remaining non-SGR escape sequences (cursor movement, OSC, charset, etc.)
  return result.replace(ANSI_RE, "");
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
  // Use fresh regex to avoid lastIndex state from global ANSI_RE
  // eslint-disable-next-line no-control-regex
  return /\x1b/.test(text);
}
