// Unified platform detection — guard for Vitest (node) environment
const ua = typeof navigator !== "undefined" ? (navigator.platform ?? "") : "";
export const IS_MAC = /Mac|iPhone|iPad|iPod/.test(ua);
export const IS_LINUX = /Linux/.test(ua);
export const IS_WINDOWS = /Win/.test(ua);

// WebKit (Tauri WKWebView on macOS) has unreliable content-visibility:auto re-layout.
const fullUA = typeof navigator !== "undefined" ? navigator.userAgent : "";
export const IS_WEBKIT = /AppleWebKit/.test(fullUA) && !/(Chrome|Chromium|Edg|OPR)/.test(fullUA);
