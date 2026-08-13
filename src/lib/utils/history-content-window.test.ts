import { describe, expect, it } from "vitest";
import { appendHistoryChunkWindow } from "./history-content-window";

describe("appendHistoryChunkWindow", () => {
  it("retains at most the two newest decoded chunks", () => {
    let state = appendHistoryChunkWindow([], [], "one", 0);
    state = appendHistoryChunkWindow(state.chunks, state.chunkStarts, "two", 256 * 1024);
    state = appendHistoryChunkWindow(state.chunks, state.chunkStarts, "three", 512 * 1024);

    expect(state.chunks).toEqual(["two", "three"]);
    expect(state.windowStart).toBe(256 * 1024);
  });

  it("uses actual chunk offsets when the preceding chunk is short", () => {
    let state = appendHistoryChunkWindow([], [], "one", 0);
    state = appendHistoryChunkWindow(state.chunks, state.chunkStarts, "two", 256 * 1024);
    state = appendHistoryChunkWindow(state.chunks, state.chunkStarts, "three", 300 * 1024);
    expect(state.windowStart).toBe(256 * 1024);
  });
});
