import { describe, expect, test } from "bun:test";

import { durationFor, isClipped } from "./bubble";

describe("durationFor", () => {
  test("permission stays until the state changes", () => {
    expect(durationFor("Bash の実行許可を待っています", "permission")).toBe(0);
  });

  test("system messages have a fixed duration", () => {
    expect(durationFor("x", "system")).toBe(6000);
  });

  test("speech duration grows with length and is clamped", () => {
    expect(durationFor("短い", "reply")).toBe(4000);
    expect(durationFor("あ".repeat(50), "assistant")).toBe(7000);
    expect(durationFor("あ".repeat(500), "reply")).toBe(12000);
  });
});

describe("isClipped", () => {
  test("clipped when the content is taller than the box", () => {
    expect(isClipped({ scrollHeight: 90, clientHeight: 60 })).toBe(true);
  });

  test("not clipped when it fits", () => {
    expect(isClipped({ scrollHeight: 60, clientHeight: 60 })).toBe(false);
  });

  test("sub-pixel rounding does not count as clipped", () => {
    // 実測値は端数を持つ。1px 未満の差でフェードを出すと、切れていないのに出てしまう
    expect(isClipped({ scrollHeight: 60.4, clientHeight: 60 })).toBe(false);
  });
});
