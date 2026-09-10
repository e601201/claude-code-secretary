// 全文ビュー(Issue #1)。吹き出しが出しきれなかった応答本文を読むための画面。
//
// 開いた時点の 1 件に固定する。読んでいる最中に中身が差し替わるのは、この Issue の
// 出発点そのものだから。最新を見たいときは開き直す(そのたびに Rust 側がイベントを飛ばす)。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { FullSpeech } from "../generated/FullSpeech";

import { headerText } from "./format";

/** Rust 側 fulltext.rs の FULLTEXT_EVENT と一致させる */
const FULLTEXT_EVENT = "secretary://fulltext";

const EMPTY_TEXT = "まだ読み返せる発言がありません。";

function byId<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element: ${id}`);
  return el as T;
}

function render(header: HTMLElement, body: HTMLElement, full: FullSpeech | null): void {
  if (!full) {
    header.textContent = "";
    body.textContent = EMPTY_TEXT;
    body.classList.add("empty");
    return;
  }
  header.textContent = headerText(full);
  body.textContent = full.text;
  body.classList.remove("empty");
  body.scrollTop = 0;
}

async function main(): Promise<void> {
  const header = byId<HTMLElement>("header");
  const body = byId<HTMLElement>("body");

  const load = async (): Promise<void> => {
    try {
      render(header, body, await invoke<FullSpeech | null>("full_speech"));
    } catch (e) {
      header.textContent = "";
      body.textContent = `控えを読めませんでした: ${String(e)}`;
      body.classList.add("empty");
    }
  };

  // ウィンドウは閉じても隠すだけなので、出し直されるたびに読み直す
  await listen(FULLTEXT_EVENT, () => void load());
  await load();
}

window.addEventListener("DOMContentLoaded", () => {
  void main();
});
