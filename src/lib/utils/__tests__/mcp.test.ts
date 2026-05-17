import { describe, it, expect } from "vitest";
import { dedupeMcpServersByName, parseServersFromResponse } from "../mcp";

describe("dedupeMcpServersByName", () => {
  it("removes duplicates while preserving first occurrence", () => {
    const result = dedupeMcpServersByName([
      { name: "context7", status: "connected", scope: "user" },
      { name: "context7", status: "failed", scope: "project" },
      { name: "github", status: "connected" },
      { name: "context7", status: "connected", scope: "local" },
    ]);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ name: "context7", status: "connected", scope: "user" });
    expect(result[1]).toEqual({ name: "github", status: "connected" });
  });

  it("returns empty array unchanged", () => {
    expect(dedupeMcpServersByName([])).toEqual([]);
  });

  it("returns single-item list unchanged", () => {
    const input = [{ name: "postgres", status: "connected" }];
    expect(dedupeMcpServersByName(input)).toEqual(input);
  });
});

describe("parseServersFromResponse", () => {
  it("dedupes parsed servers by name", () => {
    const result = parseServersFromResponse({
      servers: [
        { name: "context7", status: "connected", scope: "user" },
        { name: "context7", status: "connected", scope: "project" },
        { name: "github", status: "failed" },
      ],
    });
    expect(result).toHaveLength(2);
    expect(result[0].name).toBe("context7");
    expect(result[1].name).toBe("github");
  });

  it("returns empty array for missing arr", () => {
    expect(parseServersFromResponse({})).toEqual([]);
  });
});
