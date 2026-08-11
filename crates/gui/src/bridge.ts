import { Channel, invoke } from "@tauri-apps/api/core";

import { createDemoBridge } from "./demo";
import type {
  Bootstrap,
  DesktopBridge,
  GuiEvent,
  SessionSnapshot,
  Settings,
  ToolchainStatus,
} from "./types";

function tauriBridge(): DesktopBridge {
  return {
    bootstrap(onEvent) {
      const channel = new Channel<GuiEvent>();
      channel.onmessage = onEvent;
      return invoke<Bootstrap>("bootstrap", { onEvent: channel });
    },
    pickInputs(kind) {
      return invoke<SessionSnapshot>("pick_inputs", { kind });
    },
    pickOutputDirectory() {
      return invoke<SessionSnapshot>("pick_output_directory");
    },
    removeItems(ids) {
      return invoke<SessionSnapshot>("remove_items", { ids });
    },
    setItemEnabled(id, enabled) {
      return invoke<SessionSnapshot>("set_item_enabled", { id, enabled });
    },
    updateSettings(settings: Settings) {
      return invoke<SessionSnapshot>("update_settings", { settings });
    },
    async startBatch() {
      await invoke("start_batch");
    },
    async cancelBatch() {
      await invoke("cancel_batch");
    },
    retryItems(ids) {
      return invoke<SessionSnapshot>("retry_items", { ids });
    },
    chooseFfmpeg() {
      return invoke<ToolchainStatus>("choose_ffmpeg");
    },
    retryToolchain() {
      return invoke<ToolchainStatus>("retry_toolchain");
    },
  };
}

export function createBridge(): DesktopBridge {
  const query = new URLSearchParams(window.location.search);
  const demo = query.get("demo");
  return demo || !("__TAURI_INTERNALS__" in window)
    ? createDemoBridge(demo ?? "empty")
    : tauriBridge();
}
