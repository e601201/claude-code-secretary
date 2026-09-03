// エントリポイント。Rust 側のスナップショットを購読してキャラクターに反映する。
// 判断は Rust 側(secretary-core)が行い、ここでは描くだけ。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { SpeechBubble } from "./bubble/bubble";
import { Character } from "./character/character";
import type { SecretarySnapshot } from "./generated/SecretarySnapshot";
import { StatusPanel } from "./panel/panel";

/** Rust 側 bridge.rs の SNAPSHOT_EVENT と一致させる */
const SNAPSHOT_EVENT = "secretary://snapshot";

/** Rust 側の標準エラー出力に流す診断ログ。Tauri 外(素のブラウザ)では無視する */
function log(message: string): void {
  invoke("frontend_log", { message }).catch(() => {});
}

async function main(): Promise<void> {
  const stage = document.getElementById("stage");
  const img = document.getElementById("character") as HTMLImageElement | null;
  const badge = document.getElementById("badge");
  const bubbleEl = document.getElementById("bubble");
  const bubbleText = document.getElementById("bubble-text");
  const panelEl = document.getElementById("panel");
  const panelStatus = document.getElementById("panel-status");
  const panelSession = document.getElementById("panel-session");
  const panelTask = document.getElementById("panel-task");
  const panelTool = document.getElementById("panel-tool");
  if (
    !stage || !img || !badge || !bubbleEl || !bubbleText ||
    !panelEl || !panelStatus || !panelSession || !panelTask || !panelTool
  ) {
    return;
  }

  const character = new Character(stage, img, badge, log);
  const bubble = new SpeechBubble(bubbleEl, bubbleText, log);
  const panel = new StatusPanel(
    panelEl,
    { status: panelStatus, session: panelSession, task: panelTask, tool: panelTool },
    4000,
    log,
  );
  await character.mount();

  const apply = (snapshot: SecretarySnapshot): void => {
    character.setState(snapshot.status);
    bubble.update(snapshot);
    panel.update(snapshot);
    log(`state=${snapshot.status} message=${JSON.stringify(snapshot.message)} tool=${JSON.stringify(snapshot.current_tool)}`);
  };

  try {
    await listen<SecretarySnapshot>(SNAPSHOT_EVENT, (event) => apply(event.payload));
    // 購読前に流れたぶんを取りこぼさないよう、最後のスナップショットを取りに行く
    apply(await invoke<SecretarySnapshot>("get_snapshot"));
  } catch (e) {
    log(`snapshot bridge unavailable: ${String(e)}`);
  }
}

// 透明ウィンドウ上の右クリックメニューは誤操作のもとなので出さない
document.addEventListener("contextmenu", (e) => e.preventDefault());

window.addEventListener("DOMContentLoaded", () => {
  void main();
});
