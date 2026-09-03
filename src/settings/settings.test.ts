import { describe, expect, test } from "bun:test";

import { formatScale, joinFollow, parsePrefixes, splitFollow } from "./form";

describe("follow の往復", () => {
  test("既定値と別名は discord モード", () => {
    expect(splitFollow("discord")).toEqual({ mode: "discord", session: "" });
    expect(splitFollow("")).toEqual({ mode: "discord", session: "" });
    expect(splitFollow("channel")).toEqual({ mode: "discord", session: "" });
    expect(splitFollow("all")).toEqual({ mode: "all", session: "" });
    expect(splitFollow(" abc-123 ")).toEqual({ mode: "session", session: "abc-123" });
  });

  test("session が空なら既定に落ちる", () => {
    expect(joinFollow("session", "  ")).toBe("discord");
    expect(joinFollow("session", " abc ")).toBe("abc");
    expect(joinFollow("all", "ignored")).toBe("all");
    expect(joinFollow("discord", "ignored")).toBe("discord");
  });
});

describe("parsePrefixes", () => {
  test("空行と重複を捨て、前後の空白を落とす", () => {
    expect(parsePrefixes(" /a \n\n/b\r\n/a\n   ")).toEqual(["/a", "/b"]);
    expect(parsePrefixes("")).toEqual([]);
  });
});

test("formatScale はパーセント表示", () => {
  expect(formatScale(1)).toBe("100%");
  expect(formatScale(0.75)).toBe("75%");
  expect(formatScale(1.5)).toBe("150%");
});
