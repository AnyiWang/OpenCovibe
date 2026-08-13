export function appendHistoryChunkWindow(
  chunks: string[],
  chunkStarts: number[],
  chunk: string,
  chunkStart: number,
): { chunks: string[]; chunkStarts: number[]; windowStart: number } {
  const bounded = [...chunks, chunk];
  const boundedStarts = [...chunkStarts, chunkStart];
  if (bounded.length <= 2) {
    return { chunks: bounded, chunkStarts: boundedStarts, windowStart: boundedStarts[0] ?? 0 };
  }
  bounded.shift();
  boundedStarts.shift();
  return {
    chunks: bounded,
    chunkStarts: boundedStarts,
    windowStart: boundedStarts[0] ?? 0,
  };
}
