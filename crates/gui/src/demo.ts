import type {
  Bootstrap,
  DesktopBridge,
  GuiEvent,
  QueueItem,
  SessionSnapshot,
  Settings,
} from "./types";

const settings: Settings = {
  profile: "generic-tv",
  action: "convert",
  codec: "ac3",
  bitrate: "640k",
  channels: "keep-up-to-5.1",
  mode: "add",
  jobs: 2,
  storageProfile: "balanced",
  failurePolicy: "continue",
  dryRun: false,
  outputDirectory: "D:\\Movies\\SonicMux",
};

const plannedItems: QueueItem[] = [
  {
    id: 1,
    name: "Arrival.2016.UHD.mkv",
    inputDisplay: "D:\\Movies\\Arrival.2016.UHD.mkv",
    outputDisplay: "D:\\Movies\\SonicMux\\Arrival.2016.UHD.sonicmux.mkv",
    enabled: true,
    status: "ready",
    progressMilli: null,
    etaSeconds: null,
    plan: "Encode 1 audio track; copy video",
    error: null,
    tracks: [
      { index: 0, kind: "video", codec: "hevc", channels: null, language: null, title: "2160p HDR", default: true, action: "copy" },
      { index: 1, kind: "audio", codec: "truehd", channels: 8, language: "eng", title: "TrueHD Atmos", default: true, action: "copy+encode" },
      { index: 2, kind: "audio", codec: "ac3", channels: 6, language: "rus", title: "Dub", default: false, action: "copy" },
      { index: 3, kind: "subtitle", codec: "subrip", channels: null, language: "eng", title: "English", default: false, action: "copy" },
    ],
  },
  {
    id: 2,
    name: "The.Expanse.S01E01.mkv",
    inputDisplay: "D:\\Shows\\The.Expanse.S01E01.mkv",
    outputDisplay: "D:\\Movies\\SonicMux\\The.Expanse.S01E01.sonicmux.mkv",
    enabled: true,
    status: "compatible",
    progressMilli: null,
    etaSeconds: null,
    plan: "Already compatible",
    error: null,
    tracks: [
      { index: 0, kind: "video", codec: "h264", channels: null, language: null, title: "1080p", default: true, action: "none" },
      { index: 1, kind: "audio", codec: "ac3", channels: 6, language: "eng", title: "5.1", default: true, action: "none" },
    ],
  },
  {
    id: 3,
    name: "Moon.2009.mkv",
    inputDisplay: "D:\\Movies\\Moon.2009.mkv",
    outputDisplay: "D:\\Movies\\SonicMux\\Moon.2009.sonicmux.mkv",
    enabled: true,
    status: "failed",
    progressMilli: null,
    etaSeconds: null,
    plan: "Waiting for probe",
    error: "FFprobe could not read the container. Verify the MKV and retry.",
    tracks: [],
  },
];

function snapshot(mode: string): SessionSnapshot {
  let queue = mode === "empty" || mode === "setup" ? [] : structuredClone(plannedItems);
  if (mode === "large") {
    queue = Array.from({ length: 120 }, (_, index) => ({
      ...structuredClone(plannedItems[index % 2]),
      id: index + 1,
      name: `Movie.${String(index + 1).padStart(3, "0")}.mkv`,
    }));
  }
  if (mode === "running") {
    queue[0].status = "running";
    queue[0].progressMilli = 683;
    queue[0].etaSeconds = 74;
    queue[1].status = "queued";
  }
  if (mode === "error") {
    queue.splice(0, 2);
  }
  return {
    phase: mode === "running" ? "running" : mode === "setup" ? "toolchain-setup" : "idle",
    queue,
    settings: structuredClone(settings),
    profiles: ["generic-tv", "samsung-tv", "lg-tv"],
    canStart: mode === "planned" || mode === "large",
    progressMilli: mode === "running" ? 342 : null,
    etaSeconds: mode === "running" ? 194 : null,
    logs: [
      "FFmpeg ready from system PATH",
      "3 new MKV files queued for probe",
      "probe pass finished",
    ],
  };
}

export function createDemoBridge(mode: string): DesktopBridge {
  let current = snapshot(mode);
  let notify: ((event: GuiEvent) => void) | undefined;
  const publish = () => notify?.({ event: "snapshot", data: structuredClone(current) });
  const update = (next: SessionSnapshot) => {
    current = next;
    publish();
    return Promise.resolve(structuredClone(current));
  };
  return {
    bootstrap(onEvent) {
      notify = onEvent;
      const result: Bootstrap = {
        schema: "sonicmux.gui.v1",
        version: "0.1.0",
        toolchain: {
          available: mode !== "setup",
          source: mode === "setup" ? "missing" : "path",
          detail: mode === "setup" ? "FFmpeg and FFprobe were not found" : "FFmpeg 7.1 · system PATH",
        },
        snapshot: structuredClone(current),
      };
      return Promise.resolve(result);
    },
    pickInputs() { return update(snapshot("planned")); },
    pickOutputDirectory() { return Promise.resolve(structuredClone(current)); },
    removeItems(ids) { return update({ ...current, queue: current.queue.filter((item) => !ids.includes(item.id)) }); },
    setItemEnabled(id, enabled) {
      return update({ ...current, queue: current.queue.map((item) => item.id === id ? { ...item, enabled } : item) });
    },
    updateSettings(next) { return update({ ...current, settings: structuredClone(next) }); },
    async startBatch() {
      current = snapshot("running");
      publish();
    },
    async cancelBatch() {
      current = { ...current, phase: "idle", progressMilli: null, queue: current.queue.map((item) => item.status === "running" ? { ...item, status: "cancelled" } : item) };
      publish();
    },
    retryItems(ids) {
      return update({ ...current, queue: current.queue.map((item) => ids.includes(item.id) ? { ...item, status: "probing", error: null } : item) });
    },
    chooseFfmpeg() { return Promise.resolve({ available: true, source: "explicit", detail: "FFmpeg selected" }); },
    retryToolchain() { return Promise.resolve({ available: true, source: "path", detail: "FFmpeg found on system PATH" }); },
  };
}
