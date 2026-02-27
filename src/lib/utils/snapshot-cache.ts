/**
 * IndexedDB snapshot cache for terminal session state.
 *
 * Caches the reducer output (timeline, usage, tools, etc.) so that
 * revisiting a finished session skips getBusEvents IPC + reducer replay.
 *
 * Cache invalidation:
 * - SNAPSHOT_VERSION bump → readSnapshot rejects stale records
 * - runStatus mismatch → readSnapshot rejects
 * - resumeSession → explicit deleteSnapshot (session goes live)
 * - syncCliSession appends events → explicit deleteSnapshot
 * - IDB unavailable → graceful fallback (readSnapshot returns null)
 */
import { dbg, dbgWarn } from "$lib/utils/debug";

const DB_NAME = "opencovibe-snapshot";
const DB_VERSION = 1;
const STORE_NAME = "snapshots";

/** Bump when reducer logic changes to invalidate all cached snapshots. */
const SNAPSHOT_VERSION = 1;

interface SnapshotRecord {
  runId: string; // primary key
  version: number; // SNAPSHOT_VERSION
  runStatus: string; // terminal status at save time
  body: string; // JSON.stringify of snapshot body
  savedAt: number; // Date.now()
}

// ── Singleton DB connection ──

let dbPromise: Promise<IDBDatabase> | null = null;

function getDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise<IDBDatabase>((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME, { keyPath: "runId" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => {
      dbPromise = null; // allow retry
      reject(req.error);
    };
  });
  return dbPromise;
}

// ── Public API ──

/**
 * Read a validated snapshot.
 * Returns body string on hit, null on miss/stale.
 * Validates: version === SNAPSHOT_VERSION && runStatus === expectedStatus.
 */
export async function readSnapshot(runId: string, expectedStatus: string): Promise<string | null> {
  try {
    const db = await getDb();
    const tx = db.transaction(STORE_NAME, "readonly");
    const store = tx.objectStore(STORE_NAME);
    const record: SnapshotRecord | undefined = await new Promise((resolve, reject) => {
      const req = store.get(runId);
      req.onsuccess = () => resolve(req.result as SnapshotRecord | undefined);
      req.onerror = () => reject(req.error);
    });

    if (!record) {
      dbg("snapshot", "read:miss", { runId });
      return null;
    }

    if (record.version !== SNAPSHOT_VERSION) {
      dbg("snapshot", "read:stale", {
        runId,
        reason: "version",
        got: record.version,
        want: SNAPSHOT_VERSION,
      });
      deleteSnapshot(runId).catch(() => {});
      return null;
    }

    if (record.runStatus !== expectedStatus) {
      dbg("snapshot", "read:stale", {
        runId,
        reason: "status",
        got: record.runStatus,
        want: expectedStatus,
      });
      deleteSnapshot(runId).catch(() => {});
      return null;
    }

    dbg("snapshot", "read", { runId, hit: true, bytes: record.body.length });
    return record.body;
  } catch (err) {
    dbgWarn("snapshot", "read:error", err);
    return null;
  }
}

/** Write a snapshot. */
export async function writeSnapshot(runId: string, runStatus: string, body: string): Promise<void> {
  try {
    const db = await getDb();
    const tx = db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);
    const record: SnapshotRecord = {
      runId,
      version: SNAPSHOT_VERSION,
      runStatus,
      body,
      savedAt: Date.now(),
    };
    await new Promise<void>((resolve, reject) => {
      const req = store.put(record);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error);
    });
    dbg("snapshot", "write", { runId, runStatus, bytes: body.length });
  } catch (err) {
    dbgWarn("snapshot", "write:error", err);
  }
}

/** Delete a snapshot (e.g. after resume or sync). */
export async function deleteSnapshot(runId: string): Promise<void> {
  try {
    const db = await getDb();
    const tx = db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);
    await new Promise<void>((resolve, reject) => {
      const req = store.delete(runId);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error);
    });
    dbg("snapshot", "delete", { runId });
  } catch (err) {
    dbgWarn("snapshot", "delete:error", err);
  }
}
