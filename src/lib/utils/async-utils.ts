/**
 * Like Promise.allSettled, but with configurable concurrency.
 * Runs `fn` over `items` with at most `concurrency` concurrent tasks.
 */
export async function mapSettled<T, R>(
  items: T[],
  fn: (item: T) => Promise<R>,
  concurrency: number,
): Promise<PromiseSettledResult<R>[]> {
  const results: PromiseSettledResult<R>[] = new Array(items.length);
  let next = 0;
  const c = Math.max(1, Math.min(concurrency, items.length));
  async function worker() {
    while (next < items.length) {
      const i = next++;
      try {
        results[i] = { status: "fulfilled", value: await fn(items[i]) };
      } catch (e) {
        results[i] = { status: "rejected", reason: e };
      }
    }
  }
  await Promise.all(Array.from({ length: c }, () => worker()));
  return results;
}
