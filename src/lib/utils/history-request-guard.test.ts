import { describe, expect, it } from "vitest";
import { isCurrentContentChunk, isCurrentSubhistoryPage } from "./history-request-guard";

describe("history request guards", () => {
  it("rejects a content chunk after the viewer identity changes", () => {
    const request = {
      token: 1,
      runId: "run-1",
      generationId: "generation-1",
      contentId: "content-1",
      offset: 0,
    };
    const chunk = {
      runId: "run-1",
      generationId: "generation-1",
      contentId: "content-1",
      offset: 0,
      nextOffset: 10,
      totalBytes: 10,
      eof: true,
      dataBase64: "",
    };

    expect(isCurrentContentChunk(request, 1, chunk)).toBe(true);
    expect(
      isCurrentContentChunk(request, 1, chunk, {
        runId: "run-2",
        generationId: "generation-1",
        contentId: "content-1",
      }),
    ).toBe(false);
    expect(isCurrentContentChunk(request, 2, chunk)).toBe(false);
    expect(isCurrentContentChunk(request, 1, { ...chunk, generationId: "generation-2" })).toBe(
      false,
    );
  });

  it("rejects a subhistory page after the tool identity changes", () => {
    const request = {
      token: 3,
      runId: "run-1",
      generationId: "generation-1",
      subHistoryId: "sub-1",
    };
    const page = {
      runId: "run-1",
      generationId: "generation-1",
      subHistoryId: "sub-1",
      entries: [],
      pageCursor: "generation-1:1",
      hasMore: false,
    };

    expect(isCurrentSubhistoryPage(request, 3, page)).toBe(true);
    expect(
      isCurrentSubhistoryPage(request, 3, page, {
        runId: "run-1",
        generationId: "generation-2",
        subHistoryId: "sub-1",
      }),
    ).toBe(false);
    expect(isCurrentSubhistoryPage(request, 4, page)).toBe(false);
    expect(isCurrentSubhistoryPage(request, 3, { ...page, subHistoryId: "sub-2" })).toBe(false);
  });
});
