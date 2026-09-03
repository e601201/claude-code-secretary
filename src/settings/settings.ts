// 設定画面(Phase 11)。Rust 側の settings.rs と config.toml を読み書きする。
// 値の検証と反映は Rust 側で行い、ここではフォームと AppConfig の往復だけを扱う。

import { invoke } from "@tauri-apps/api/core";

import type { AppConfig } from "../generated/AppConfig";
import type { SettingsInfo } from "../generated/SettingsInfo";
import { formatScale, joinFollow, parsePrefixes, splitFollow, type FollowMode } from "./form";

function byId<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element: ${id}`);
  return el as T;
}

function readNumber(input: HTMLInputElement, fallback: number): number {
  const v = Number(input.value);
  return Number.isFinite(v) ? v : fallback;
}

async function main(): Promise<void> {
  const form = byId<HTMLFormElement>("form");
  const scale = byId<HTMLInputElement>("scale");
  const scaleOut = byId<HTMLOutputElement>("scale-out");
  const followSession = byId<HTMLInputElement>("follow-session");
  const cwdPrefixes = byId<HTMLTextAreaElement>("cwd-prefixes");
  const notify = byId<HTMLInputElement>("notify");
  const autostart = byId<HTMLInputElement>("autostart");
  const autostartNote = byId<HTMLElement>("autostart-note");
  const port = byId<HTMLInputElement>("port");
  const successHold = byId<HTMLInputElement>("success-hold");
  const errorHold = byId<HTMLInputElement>("error-hold");
  const stale = byId<HTMLInputElement>("stale");
  const maxChars = byId<HTMLInputElement>("max-chars");
  const configPath = byId<HTMLElement>("config-path");
  const personaPath = byId<HTMLElement>("persona-path");
  const status = byId<HTMLElement>("status");
  const save = byId<HTMLButtonElement>("save");
  const followRadios = Array.from(
    form.querySelectorAll<HTMLInputElement>('input[name="follow-mode"]'),
  );

  let current: AppConfig | null = null;

  const setStatus = (text: string, kind: "ok" | "error" | "" = ""): void => {
    status.textContent = text;
    status.dataset.kind = kind;
  };

  const fill = (info: SettingsInfo): void => {
    current = info.config;
    const c = info.config;
    scale.value = String(c.scale);
    scaleOut.textContent = formatScale(c.scale);
    const f = splitFollow(c.follow);
    for (const r of followRadios) r.checked = r.value === f.mode;
    followSession.value = f.session;
    cwdPrefixes.value = c.cwd_prefixes.join("\n");
    notify.checked = c.notify_on_waiting;
    autostart.checked = info.autostart_enabled;
    autostart.disabled = !info.autostart_available;
    autostartNote.textContent = info.autostart_available ? "" : "ビルド版でのみ設定できます";
    port.value = String(c.port);
    successHold.value = String(c.success_hold_secs);
    errorHold.value = String(c.error_hold_secs);
    stale.value = String(c.stale_after_secs);
    maxChars.value = String(c.max_message_chars);
    configPath.textContent = info.config_path;
    personaPath.textContent = info.persona_path;
  };

  const collect = (): AppConfig => {
    const base = current ?? {
      port: 47831,
      follow: "discord",
      cwd_prefixes: [],
      success_hold_secs: 3,
      error_hold_secs: 5,
      stale_after_secs: 1800,
      max_message_chars: 120,
      scale: 1,
      notify_on_waiting: true,
    };
    const mode = (followRadios.find((r) => r.checked)?.value ?? "discord") as FollowMode;
    return {
      ...base,
      port: Math.round(readNumber(port, base.port)),
      follow: joinFollow(mode, followSession.value),
      cwd_prefixes: parsePrefixes(cwdPrefixes.value),
      success_hold_secs: readNumber(successHold, base.success_hold_secs),
      error_hold_secs: readNumber(errorHold, base.error_hold_secs),
      stale_after_secs: Math.round(readNumber(stale, Number(base.stale_after_secs))),
      max_message_chars: Math.round(readNumber(maxChars, Number(base.max_message_chars))),
      scale: readNumber(scale, base.scale),
      notify_on_waiting: notify.checked,
    };
  };

  scale.addEventListener("input", () => {
    scaleOut.textContent = formatScale(readNumber(scale, 1));
  });
  followSession.addEventListener("focus", () => {
    for (const r of followRadios) r.checked = r.value === "session";
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    save.disabled = true;
    setStatus("保存しています…");
    try {
      const needsRestart = await invoke<boolean>("save_config", { config: collect() });
      const info = await invoke<SettingsInfo>("settings_info");
      fill(info);
      setStatus(
        needsRestart ? "保存しました。ポートの変更はアプリの再起動後に反映されます" : "保存しました",
        "ok",
      );
    } catch (err) {
      setStatus(String(err), "error");
    } finally {
      save.disabled = false;
    }
  });

  autostart.addEventListener("change", async () => {
    try {
      autostart.checked = await invoke<boolean>("set_autostart", { enabled: autostart.checked });
      setStatus(autostart.checked ? "ログイン時に起動します" : "ログイン時の起動をやめました", "ok");
    } catch (err) {
      autostart.checked = !autostart.checked;
      setStatus(String(err), "error");
    }
  });

  for (const button of form.querySelectorAll<HTMLButtonElement>("button[data-open]")) {
    button.addEventListener("click", () => {
      invoke("open_path", { which: button.dataset.open }).catch((err) => setStatus(String(err), "error"));
    });
  }
  byId<HTMLButtonElement>("reload-persona").addEventListener("click", async () => {
    try {
      await invoke("reload_persona");
      setStatus("口調の辞書を読み直しました", "ok");
    } catch (err) {
      setStatus(String(err), "error");
    }
  });

  try {
    fill(await invoke<SettingsInfo>("settings_info"));
  } catch (err) {
    setStatus(`設定を読めません: ${String(err)}`, "error");
  }
}

window.addEventListener("DOMContentLoaded", () => {
  void main();
});
