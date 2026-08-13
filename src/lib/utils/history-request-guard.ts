import type { HistoryContentChunk, SubHistoryPage } from "$lib/types";

export interface ContentChunkRequest {
  token: number;
  runId: string;
  generationId: string;
  contentId: string;
  offset: number;
}

export function isCurrentContentChunk(
  request: ContentChunkRequest,
  currentToken: number,
  chunk: HistoryContentChunk,
  current: Pick<ContentChunkRequest, "runId" | "generationId" | "contentId"> = request,
): boolean {
  return (
    request.token === currentToken &&
    current.runId === request.runId &&
    current.generationId === request.generationId &&
    current.contentId === request.contentId &&
    chunk.runId === request.runId &&
    chunk.generationId === request.generationId &&
    chunk.contentId === request.contentId &&
    chunk.offset === request.offset
  );
}

export interface SubhistoryRequest {
  token: number;
  runId: string;
  generationId: string;
  subHistoryId: string;
}

export function isCurrentSubhistoryPage(
  request: SubhistoryRequest,
  currentToken: number,
  page: SubHistoryPage,
  current: Pick<SubhistoryRequest, "runId" | "generationId" | "subHistoryId"> = request,
): boolean {
  return (
    request.token === currentToken &&
    current.runId === request.runId &&
    current.generationId === request.generationId &&
    current.subHistoryId === request.subHistoryId &&
    page.runId === request.runId &&
    page.generationId === request.generationId &&
    page.subHistoryId === request.subHistoryId
  );
}
