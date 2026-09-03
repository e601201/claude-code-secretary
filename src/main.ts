// Phase 2: キャラクターを表示するだけ。状態の描画は Phase 3 以降。

import { invoke } from "@tauri-apps/api/core";

const CHARACTER_IMAGE = "/character/base.png";
const PLACEHOLDER_IMAGE = "/character/placeholder.svg";

/** Rust 側の標準エラー出力に流す診断ログ。Tauri 外(素のブラウザ)では無視する */
function log(message: string): void {
  invoke("frontend_log", { message }).catch(() => {});
}

function mountCharacter(): void {
  const img = document.getElementById("character") as HTMLImageElement | null;
  if (!img) return;

  // public/character/base.png が無ければ仮のキャラクターに切り替える
  img.addEventListener("error", () => {
    if (img.src.endsWith(PLACEHOLDER_IMAGE)) {
      log(`placeholder failed to load: ${img.src}`);
      return;
    }
    log(`character image not found, falling back to placeholder`);
    img.src = PLACEHOLDER_IMAGE;
  });
  img.addEventListener("load", () => {
    log(`character loaded: ${img.src} (${img.naturalWidth}x${img.naturalHeight}), window ${window.innerWidth}x${window.innerHeight}`);
  });
  img.src = CHARACTER_IMAGE;
  log(`mounting character from ${CHARACTER_IMAGE}`);
}

// 透明ウィンドウ上の右クリックメニューは誤操作のもとなので出さない
document.addEventListener("contextmenu", (e) => e.preventDefault());

window.addEventListener("DOMContentLoaded", mountCharacter);
