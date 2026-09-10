// 全文ビューの見出しの組み立て。DOM にも Tauri にも触らない純粋な関数だけを置く。

import type { FullSpeech } from "../generated/FullSpeech";

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * 受け取った時刻。日付を必ず添える。
 *
 * 「今日なら時刻だけ」にすると、表示が読んだ瞬間からの相対になる。ウィンドウは開きっぱなしに
 * できるので、日付をまたいだ途端に `14:32` が昨日を指す嘘になる。相対表示を避けたのと同じ理由。
 */
export function formatReceivedAt(ms: number): string {
  const at = new Date(ms);
  const date = `${at.getMonth() + 1}/${at.getDate()}`;
  return `${date} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** 見出し。吹き出しの主とは一致しないので、どのセッションのいつの発言かを必ず出す */
export function headerText(full: FullSpeech): string {
  const parts = [full.session_label, formatReceivedAt(full.received_at_ms)];
  if (full.truncated) parts.push("長すぎるためここで切れています");
  return parts.join(" · ");
}
