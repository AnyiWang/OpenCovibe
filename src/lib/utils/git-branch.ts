import { dbg, dbgWarn } from "$lib/utils/debug";

type FetchBranch = (cwd: string) => Promise<string>;

export function createGitBranchPoller(fetchBranch: FetchBranch) {
  let reqId = 0;
  let current = "";
  let suppressedKey = "";

  function refresh(cwd: string): Promise<string> {
    const id = ++reqId;
    if (!cwd) {
      current = "";
      suppressedKey = "";
      dbg("git-branch", "skip: no cwd");
      return Promise.resolve("");
    }
    return fetchBranch(cwd)
      .then((branch) => {
        if (id !== reqId) {
          dbg("git-branch", "discard stale", { id, current: reqId, cwd });
          return current;
        }
        current = branch;
        suppressedKey = "";
        dbg("git-branch", "updated", { branch, cwd });
        return branch;
      })
      .catch((err) => {
        if (id !== reqId) return current;
        const category = String(err).split("\n")[0]; // handle non-Error types
        const errKey = `${cwd}:${category}`;
        if (errKey !== suppressedKey) {
          dbgWarn("git-branch", "fetch error", { cwd, error: category });
          suppressedKey = errKey;
        }
        current = "";
        return "";
      });
  }

  return {
    refresh,
    get current() {
      return current;
    },
    get reqId() {
      return reqId;
    },
  };
}
