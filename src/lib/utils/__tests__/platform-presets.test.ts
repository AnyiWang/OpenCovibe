import { describe, it, expect } from "vitest";
import { expandModelsToTiers, compressModelsFromTiers } from "../platform-presets";

describe("expandModelsToTiers", () => {
  it("undefined → all empty", () => {
    expect(expandModelsToTiers(undefined)).toEqual(["", "", ""]);
  });

  it("empty array → all empty", () => {
    expect(expandModelsToTiers([])).toEqual(["", "", ""]);
  });

  it("1 model → all same", () => {
    expect(expandModelsToTiers(["m"])).toEqual(["m", "m", "m"]);
  });

  it("2 models → [0]=opus+sonnet, [1]=haiku", () => {
    expect(expandModelsToTiers(["main", "eco"])).toEqual(["main", "main", "eco"]);
  });

  it("3 models → positional", () => {
    expect(expandModelsToTiers(["o", "s", "h"])).toEqual(["o", "s", "h"]);
  });

  it("3 models with empty strings preserved", () => {
    expect(expandModelsToTiers(["", "s", ""])).toEqual(["", "s", ""]);
  });
});

describe("compressModelsFromTiers", () => {
  it("all empty → undefined", () => {
    expect(compressModelsFromTiers("", "", "")).toBeUndefined();
  });

  it("all whitespace → undefined", () => {
    expect(compressModelsFromTiers("  ", " ", "  ")).toBeUndefined();
  });

  it("only sonnet → ['', 's', '']", () => {
    expect(compressModelsFromTiers("", "s", "")).toEqual(["", "s", ""]);
  });

  it("sonnet + haiku → ['', 's', 'h']", () => {
    expect(compressModelsFromTiers("", "s", "h")).toEqual(["", "s", "h"]);
  });

  it("all three → ['o', 's', 'h']", () => {
    expect(compressModelsFromTiers("o", "s", "h")).toEqual(["o", "s", "h"]);
  });

  it("trims whitespace", () => {
    expect(compressModelsFromTiers(" o ", " s ", " h ")).toEqual(["o", "s", "h"]);
  });

  it("opus + sonnet, empty haiku → ['o', 's', '']", () => {
    expect(compressModelsFromTiers("o", "s", "")).toEqual(["o", "s", ""]);
  });

  it("only opus → ['o', '', '']", () => {
    expect(compressModelsFromTiers("o", "", "")).toEqual(["o", "", ""]);
  });
});
