/**
 * Transport abstraction layer.
 *
 * Detects Tauri desktop vs browser environment and returns the appropriate
 * transport implementation. The singleton is cached after first call.
 */
import { dbg } from "$lib/utils/debug";
import { TauriTransport } from "./tauri";
import { WsTransport } from "./websocket";

export interface Transport {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
  isDesktop(): boolean;
  /** Subscribe to a run's real-time events (WS only, no-op on desktop) */
  subscribeRun(runId: string, lastSeq?: number, recovery?: "history" | "live"): Promise<void>;
  /** Unsubscribe from a run's events (WS only, no-op on desktop) */
  unsubscribeRun(runId: string): void;
}

let _instance: Transport | null = null;

export function getTransport(): Transport {
  if (!_instance) {
    const isTauri = typeof window !== "undefined" && !!(window as any).__TAURI_INTERNALS__;

    _instance = isTauri ? new TauriTransport() : new WsTransport();

    dbg("transport", "initialized", { type: _instance.isDesktop() ? "tauri" : "websocket" });
  }
  return _instance;
}
