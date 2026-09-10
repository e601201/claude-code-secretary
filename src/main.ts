// エントリポイント。Rust 側のスナップショットを購読してキャラクターに反映する。
// 判断は Rust 側(secretary-core)が行い、ここでは描くだけ。
// Phase 10: 入力欄から Claude Code へ指示を送り、中継された権限要求に吹き出しから答える。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { SpeechBubble } from "./bubble/bubble";
import { Character } from "./character/character";
import { Composer } from "./composer/composer";
import type { SecretarySnapshot } from "./generated/SecretarySnapshot";
import type { SheetStatus } from "./generated/SheetStatus";
import { StatusPanel } from "./panel/panel";

/** Rust 側 bridge.rs の SNAPSHOT_EVENT と一致させる */
const SNAPSHOT_EVENT = "secretary://snapshot";
/** Rust 側 lib.rs の PANEL_PIN_EVENT と一致させる */
const PANEL_PIN_EVENT = "secretary://panel-pin";
/** Rust 側 sprite.rs の SHEET_EVENT と一致させる */
const SHEET_EVENT = "secretary://sheet";
/** Rust 側 lib.rs の COMPOSER_EVENT と一致させる */
const COMPOSER_EVENT = "secretary://composer";
/** ウィンドウの基準幅。Rust 側 lib.rs の BASE_WIDTH と一致させる */
const BASE_WIDTH = 280;

/** ウィンドウが基準サイズの何倍かを幅から読み、ステージ全体を同じ倍率にする */
function applyZoom(stage: HTMLElement): void {
  const zoom = Math.max(0.1, window.innerWidth / BASE_WIDTH);
  stage.style.setProperty("zoom", zoom.toFixed(3));
}

/** Rust 側の標準エラー出力に流す診断ログ。Tauri 外(素のブラウザ)では無視する */
function log(message: string): void {
  invoke("frontend_log", { message }).catch(() => {});
}

function byId<T extends HTMLElement>(id: string): T | null {
  return document.getElementById(id) as T | null;
}

async function main(): Promise<void> {
  const stage = byId<HTMLElement>("stage");
  const characterEl = byId<HTMLElement>("character");
  const badge = byId<HTMLElement>("badge");
  const bubbleEl = byId<HTMLElement>("bubble");
  const bubbleText = byId<HTMLElement>("bubble-text");
  const bubbleDetail = byId<HTMLElement>("bubble-detail");
  const bubbleActions = byId<HTMLElement>("bubble-actions");
  const permAllow = byId<HTMLButtonElement>("perm-allow");
  const permDeny = byId<HTMLButtonElement>("perm-deny");
  const panelEl = byId<HTMLElement>("panel");
  const panelStatus = byId<HTMLElement>("panel-status");
  const panelSession = byId<HTMLElement>("panel-session");
  const panelTask = byId<HTMLElement>("panel-task");
  const panelTool = byId<HTMLElement>("panel-tool");
  const composerForm = byId<HTMLFormElement>("composer");
  const composerInput = byId<HTMLInputElement>("composer-input");
  const talkButton = byId<HTMLButtonElement>("talk-button");
  if (
    !stage || !characterEl || !badge || !bubbleEl || !bubbleText || !bubbleDetail || !bubbleActions ||
    !permAllow || !permDeny || !panelEl || !panelStatus || !panelSession || !panelTask ||
    !panelTool || !composerForm || !composerInput || !talkButton
  ) {
    return;
  }

  applyZoom(stage);
  window.addEventListener("resize", () => applyZoom(stage));

  const character = new Character(stage, characterEl, badge, log);
  const bubble = new SpeechBubble(bubbleEl, bubbleText, log, {
    detail: bubbleDetail,
    actions: bubbleActions,
    allow: permAllow,
    deny: permDeny,
  });
  const panel = new StatusPanel(
    panelEl,
    { status: panelStatus, session: panelSession, task: panelTask, tool: panelTool },
    4000,
    log,
  );

  // 入力欄やボタンが出ている間は、ウィンドウ全体でクリックを受ける(Rust 側の cursor_watch が見る)
  let lastInteractive: boolean | null = null;
  const syncInteractive = (): void => {
    const extended = composer.isOpen || bubble.hasActions;
    if (extended === lastInteractive) return;
    lastInteractive = extended;
    invoke("set_interactive", { extended }).catch((e) => log(`set_interactive failed: ${String(e)}`));
  };

  const composer = new Composer(
    composerForm,
    composerInput,
    async (text) => {
      try {
        const label = await invoke<string>("send_prompt", { text });
        log(`sent to ${label}: ${JSON.stringify(text)}`);
        bubble.show(`${label} へ送りました`, "assistant", 2500);
      } catch (e) {
        bubble.show(String(e), "system", 8000);
        throw e;
      }
    },
    (open) => {
      stage.classList.toggle("composing", open);
      syncInteractive();
    },
  );

  bubble.onPermission(async (allow, permission, sessionId) => {
    try {
      await invoke("respond_permission", {
        sessionId,
        requestId: permission.request_id,
        allow,
      });
    } catch (e) {
      bubble.show(String(e), "system", 8000);
      throw e;
    }
  });

  talkButton.addEventListener("click", (e) => {
    e.stopPropagation();
    composer.toggle();
  });
  // キャラクターのダブルクリックでも開く(ドラッグ領域の都合で発火しない環境もある)
  stage.addEventListener("dblclick", (e) => {
    const target = e.target as HTMLElement | null;
    if (target?.closest("#composer, #bubble, #talk-button")) return;
    composer.toggle();
  });

  // 立ち絵。Rust 側が検めた結果を受け取り、使えなければプレースホルダーで動く
  void listen<SheetStatus>(SHEET_EVENT, (event) => character.setSheet(event.payload));
  character.setSheet(await invoke<SheetStatus>("sheet_status").catch(() => null));

  const apply = (snapshot: SecretarySnapshot): void => {
    character.setState(snapshot.status);
    bubble.update(snapshot);
    panel.update(snapshot);
    syncInteractive();
    log(`state=${snapshot.status} message=${JSON.stringify(snapshot.message)} tool=${JSON.stringify(snapshot.current_tool)}`);
  };

  try {
    await listen<SecretarySnapshot>(SNAPSHOT_EVENT, (event) => apply(event.payload));
    await listen<boolean>(PANEL_PIN_EVENT, (event) => panel.setPinned(event.payload));
    await listen<boolean>(COMPOSER_EVENT, (event) => {
      if (event.payload) composer.open();
      else composer.close();
    });
    // 購読前に流れたぶんを取りこぼさないよう、最後のスナップショットを取りに行く
    apply(await invoke<SecretarySnapshot>("get_snapshot"));
  } catch (e) {
    log(`snapshot bridge unavailable: ${String(e)}`);
  }
}

// 右クリックはブラウザ標準のメニューではなく、Rust 側のネイティブメニューを出す
document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  invoke("show_context_menu").catch((err) => log(`context menu failed: ${String(err)}`));
});

window.addEventListener("DOMContentLoaded", () => {
  void main();
});
