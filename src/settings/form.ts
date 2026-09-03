// 設定画面のフォームと AppConfig の間の純粋な変換。DOM に触れないのでテストしやすい。

export type FollowMode = "discord" | "all" | "session";

/** `follow` の文字列をフォームのラジオとテキストに分ける */
export function splitFollow(follow: string): { mode: FollowMode; session: string } {
  const trimmed = follow.trim();
  if (trimmed === "" || trimmed === "discord" || trimmed === "channel") {
    return { mode: "discord", session: "" };
  }
  if (trimmed === "all") return { mode: "all", session: "" };
  return { mode: "session", session: trimmed };
}

/** ラジオとテキストから `follow` の文字列へ戻す。session が空なら既定に落とす */
export function joinFollow(mode: FollowMode, session: string): string {
  switch (mode) {
    case "all":
      return "all";
    case "session":
      return session.trim() || "discord";
    case "discord":
      return "discord";
  }
}

/** テキストエリア(1 行 1 つ)を前方一致リストにする。空行と重複は捨てる */
export function parsePrefixes(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || seen.has(line)) continue;
    seen.add(line);
    out.push(line);
  }
  return out;
}

export function formatScale(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}
