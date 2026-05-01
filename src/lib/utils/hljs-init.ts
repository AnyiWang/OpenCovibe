/**
 * Centralized highlight.js language registration. Importing this module ensures all
 * supported languages are registered with the shared `highlight.js/lib/core` instance.
 *
 * Why a dedicated module: `markdown.ts` used to register languages as a side effect, and
 * `HighlightedCode.svelte` relied on `import "$lib/utils/markdown"` (side-effect-only).
 * That was unreliable across module evaluation orders, causing `hljs.getLanguage("rust")`
 * to return null at the time HighlightedCode tried to highlight a `.rs` file → fallback
 * to `highlightAuto` (~10x slower). Importing this module by reference (used export)
 * guarantees registration before highlight calls.
 */
import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import css from "highlight.js/lib/languages/css";
import xml from "highlight.js/lib/languages/xml";
import markdown from "highlight.js/lib/languages/markdown";
import yaml from "highlight.js/lib/languages/yaml";
import sql from "highlight.js/lib/languages/sql";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import cpp from "highlight.js/lib/languages/cpp";
import diff from "highlight.js/lib/languages/diff";
import shell from "highlight.js/lib/languages/shell";

let initialized = false;

/** Register all supported languages with hljs. Idempotent — safe to call multiple times. */
export function initHljs(): typeof hljs {
  if (initialized) return hljs;
  hljs.registerLanguage("javascript", javascript);
  hljs.registerLanguage("js", javascript);
  hljs.registerLanguage("typescript", typescript);
  hljs.registerLanguage("ts", typescript);
  hljs.registerLanguage("python", python);
  hljs.registerLanguage("py", python);
  hljs.registerLanguage("rust", rust);
  hljs.registerLanguage("rs", rust);
  hljs.registerLanguage("bash", bash);
  hljs.registerLanguage("sh", bash);
  hljs.registerLanguage("json", json);
  hljs.registerLanguage("css", css);
  hljs.registerLanguage("html", xml);
  hljs.registerLanguage("xml", xml);
  hljs.registerLanguage("markdown", markdown);
  hljs.registerLanguage("md", markdown);
  hljs.registerLanguage("yaml", yaml);
  hljs.registerLanguage("yml", yaml);
  hljs.registerLanguage("sql", sql);
  hljs.registerLanguage("go", go);
  hljs.registerLanguage("java", java);
  hljs.registerLanguage("cpp", cpp);
  hljs.registerLanguage("c", cpp);
  hljs.registerLanguage("diff", diff);
  hljs.registerLanguage("shell", shell);
  initialized = true;
  return hljs;
}

/** Pre-initialized hljs instance — `import { hljs } from "$lib/utils/hljs-init"` is safe. */
export { hljs };
initHljs();
