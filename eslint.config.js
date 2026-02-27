import js from "@eslint/js";
import ts from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import prettier from "eslint-config-prettier";
import globals from "globals";

export default ts.config(
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs["flat/recommended"],
  prettier,
  ...svelte.configs["flat/prettier"],
  {
    languageOptions: {
      globals: {
        ...globals.browser,
      },
      parserOptions: {
        extraFileExtensions: [".svelte"],
      },
    },
    rules: {
      // Allow explicit any in a codebase that wraps dynamic CLI JSON
      "@typescript-eslint/no-explicit-any": "off",
      // Unused vars: allow _ prefix and args (common in callbacks)
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      // Allow empty functions (common in noop callbacks)
      "@typescript-eslint/no-empty-function": "off",
      // Svelte: allow @html (sanitized via DOMPurify)
      "svelte/no-at-html-tags": "warn",
      // Disable noisy Svelte rules that don't catch real bugs:
      // - require-each-key: 47 violations, adding keys is a significant refactor
      // - no-navigation-without-resolve: not applicable to Tauri (no server routing)
      // - prefer-svelte-reactivity: false positives in reducer pattern (manually managed state)
      // - no-useless-children-snippet: cosmetic, not a bug source
      // - prefer-writable-derived: cosmetic refactor suggestion
      "svelte/require-each-key": "off",
      "svelte/no-navigation-without-resolve": "off",
      "svelte/prefer-svelte-reactivity": "off",
      "svelte/no-useless-children-snippet": "off",
      "svelte/prefer-writable-derived": "off",
    },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: {
      parserOptions: {
        parser: ts.parser,
      },
    },
  },
  {
    ignores: [
      "build/",
      ".svelte-kit/",
      "src-tauri/",
      "node_modules/",
      "vite.config.ts",
      "vitest.config.ts",
      "svelte.config.js",
      "postcss.config.js",
      "tailwind.config.ts",
    ],
  },
);
