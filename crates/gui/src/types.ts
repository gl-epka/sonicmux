export type AppPhase =
  | "toolchain-setup"
  | "idle"
  | "probing"
  | "running"
  | "cancelling";

export interface ToolchainStatus {
  available: boolean;
  source: string;
  detail: string;
}

export interface Settings {
  profile: string;
  action: "convert" | "remux";
  codec: "ac3" | "eac3" | "aac";
  bitrate: string;
  channels: "keep-up-to-5.1" | "stereo" | "5.1";
  mode: "add" | "replace" | "only-new";
  jobs: number;
  storageProfile: "hdd" | "balanced" | "nvme";
  failurePolicy: "continue" | "fail-fast";
  dryRun: boolean;
  outputDirectory: string | null;
}

export interface Track {
  index: number;
  kind: string;
  codec: string;
  channels: number | null;
  language: string | null;
  title: string | null;
  default: boolean;
  action: string;
}

export interface QueueItem {
  id: number;
  name: string;
  inputDisplay: string;
  outputDisplay: string;
  enabled: boolean;
  status: string;
  progressMilli: number | null;
  etaSeconds: number | null;
  plan: string;
  error: string | null;
  tracks: Track[];
}

export interface SessionSnapshot {
  phase: AppPhase;
  queue: QueueItem[];
  settings: Settings;
  profiles: string[];
  canStart: boolean;
  progressMilli: number | null;
  etaSeconds: number | null;
  logs: string[];
}

export interface Bootstrap {
  schema: string;
  version: string;
  toolchain: ToolchainStatus;
  snapshot: SessionSnapshot;
}

export type GuiEvent =
  | { event: "snapshot"; data: SessionSnapshot }
  | { event: "notice"; data: { level: string; message: string } }
  | { event: "menu"; data: { action: string } };

export interface DesktopBridge {
  bootstrap(onEvent: (event: GuiEvent) => void): Promise<Bootstrap>;
  pickInputs(kind: "files" | "directory"): Promise<SessionSnapshot>;
  pickOutputDirectory(): Promise<SessionSnapshot>;
  removeItems(ids: number[]): Promise<SessionSnapshot>;
  setItemEnabled(id: number, enabled: boolean): Promise<SessionSnapshot>;
  updateSettings(settings: Settings): Promise<SessionSnapshot>;
  startBatch(): Promise<void>;
  cancelBatch(): Promise<void>;
  retryItems(ids: number[]): Promise<SessionSnapshot>;
  chooseFfmpeg(): Promise<ToolchainStatus>;
  retryToolchain(): Promise<ToolchainStatus>;
}
