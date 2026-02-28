// Unified platform detection — guard for Vitest (node) environment
const ua = typeof navigator !== "undefined" ? (navigator.platform ?? "") : "";
export const IS_MAC = /Mac|iPhone|iPad|iPod/.test(ua);
export const IS_LINUX = /Linux/.test(ua);
export const IS_WINDOWS = /Win/.test(ua);
