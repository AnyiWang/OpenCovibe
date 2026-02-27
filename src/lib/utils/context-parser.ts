/**
 * Shared parser for Claude CLI `/context` markdown output.
 * Used by ContextUsageGrid (inline display) and ContextHistoryPanel (sidebar tracking).
 */
import { dbg } from "$lib/utils/debug";

// ── Types ──

export interface ContextCategory {
  name: string;
  tokens: string;
  percentage: number;
}

export interface SubTable {
  title: string;
  headers: string[];
  rows: string[][];
}

export interface ContextData {
  model: string;
  usedTokens: string;
  maxTokens: string;
  percentage: number;
  categories: ContextCategory[];
  subTables: SubTable[];
}

export interface ContextDelta {
  pctDelta: number;
  categoryDeltas: Array<{
    name: string;
    pctBefore: number;
    pctAfter: number;
    pctDelta: number;
  }>;
}

// ── Parser ──

export function parseContextMarkdown(md: string): ContextData | null {
  try {
    // Accept both bold (**Model:**) and plain (Model:) formats
    const modelMatch = md.match(/\*?\*?Model:\*?\*?\s*(.+)/);
    const tokensMatch = md.match(/\*?\*?Tokens:\*?\*?\s*(\S+)\s*\/\s*(\S+)\s*\((\d+)%\)/);
    if (!modelMatch || !tokensMatch) return null;

    const model = modelMatch[1].trim();
    const usedTokens = tokensMatch[1];
    const maxTokens = tokensMatch[2];
    const percentage = parseInt(tokensMatch[3], 10);

    const categories: ContextCategory[] = [];
    const rowRegex = /^\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*([\d.]+)%\s*\|/gm;
    let match;
    while ((match = rowRegex.exec(md)) !== null) {
      const name = match[1].trim();
      if (name === "Category" || name.startsWith("---")) continue;
      categories.push({
        name,
        tokens: match[2].trim(),
        percentage: parseFloat(match[3]),
      });
    }

    if (categories.length === 0) return null;

    // Parse sub-tables (### MCP Tools, ### Memory Files, ### Skills, etc.)
    const subTables: SubTable[] = [];
    const sectionRegex = /^###\s+(.+)/gm;
    let sectionMatch;
    while ((sectionMatch = sectionRegex.exec(md)) !== null) {
      const title = sectionMatch[1].trim();
      if (title === "Estimated usage by category") continue;
      const startIdx = sectionMatch.index + sectionMatch[0].length;
      const nextSection = md.indexOf("\n###", startIdx);
      const sectionText = md.slice(startIdx, nextSection === -1 ? undefined : nextSection);

      const tableRowRegex = /^\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|\s*([^|]+?)\s*\|/gm;
      const headers: string[] = [];
      const rows: string[][] = [];
      let rowMatch;
      while ((rowMatch = tableRowRegex.exec(sectionText)) !== null) {
        const c1 = rowMatch[1].trim();
        if (c1.startsWith("---")) continue;
        if (headers.length === 0) {
          headers.push(c1, rowMatch[2].trim(), rowMatch[3].trim());
        } else {
          rows.push([c1, rowMatch[2].trim(), rowMatch[3].trim()]);
        }
      }
      if (rows.length > 0) {
        subTables.push({ title, headers, rows });
      }
    }

    dbg("context-parser", "parsed", {
      model,
      percentage,
      categories: categories.length,
      subTables: subTables.length,
    });
    return { model, usedTokens, maxTokens, percentage, categories, subTables };
  } catch {
    return null;
  }
}

// ── Color + Icon mapping ──

export const CATEGORY_COLORS: Record<string, string> = {
  "System prompt": "#a78bfa",
  "System tools": "#f87171",
  "System tools (deferred)": "#fb923c",
  "MCP tools": "#34d399",
  "MCP tools (deferred)": "#2dd4bf",
  "Custom agents": "#60a5fa",
  "Memory files": "#fbbf24",
  Skills: "#facc15",
  Messages: "#f472b6",
};

export function getColor(name: string): string {
  if (name === "Free space") return "#6b7280";
  if (name === "Autocompact buffer") return "#4b5563";
  return CATEGORY_COLORS[name] ?? "#9ca3af";
}

export function getIcon(name: string): string {
  if (name === "Free space") return "▢";
  if (name === "Autocompact buffer") return "⊠";
  return "⛁";
}

// ── Delta computation ──

export function computeContextDelta(prev: ContextData, curr: ContextData): ContextDelta {
  const pctDelta = curr.percentage - prev.percentage;

  // Build lookup for prev categories
  const prevMap = new Map(prev.categories.map((c) => [c.name, c.percentage]));
  const currMap = new Map(curr.categories.map((c) => [c.name, c.percentage]));

  // Union of all category names
  const allNames = new Set([...prevMap.keys(), ...currMap.keys()]);

  const categoryDeltas: ContextDelta["categoryDeltas"] = [];
  for (const name of allNames) {
    const pctBefore = prevMap.get(name) ?? 0;
    const pctAfter = currMap.get(name) ?? 0;
    if (pctBefore !== pctAfter) {
      categoryDeltas.push({ name, pctBefore, pctAfter, pctDelta: pctAfter - pctBefore });
    }
  }

  return { pctDelta, categoryDeltas };
}
