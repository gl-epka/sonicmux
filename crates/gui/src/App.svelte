<script lang="ts">
  import { onMount } from "svelte";
  import {
    AlertTriangle,
    Check,
    ChevronRight,
    CircleStop,
    Clock3,
    FilePlus2,
    Film,
    FolderOpen,
    Gauge,
    HardDrive,
    ListMusic,
    Play,
    RefreshCw,
    RotateCcw,
    Settings2,
    Trash2,
    Wrench,
    X,
  } from "@lucide/svelte";

  import { createBridge } from "./bridge";
  import type { QueueItem, SessionSnapshot, Settings, ToolchainStatus } from "./types";

  const bridge = createBridge();
  const initialSettings: Settings = {
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
    outputDirectory: null,
  };
  const initialSnapshot: SessionSnapshot = {
    phase: "idle",
    queue: [],
    settings: initialSettings,
    profiles: ["generic-tv"],
    canStart: false,
    progressMilli: null,
    etaSeconds: null,
    logs: [],
  };

  let snapshot = initialSnapshot;
  let toolchain: ToolchainStatus = { available: false, source: "checking", detail: "Checking FFmpeg…" };
  let version = "";
  let selectedId: number | null = null;
  let activeTab: "tracks" | "settings" | "logs" = "tracks";
  let loading = true;
  let notice = "";
  let noticeLevel = "info";
  let selected: QueueItem | undefined;
  let enabledCount = 0;
  let active = false;
  let queueScrollTop = 0;
  let queueViewportHeight = 480;
  let virtualStart = 0;
  let virtualEnd = 0;
  let visibleQueue: QueueItem[] = [];
  let virtualized = false;

  const virtualRowHeight = 97;
  const virtualOverscan = 5;

  $: selected = snapshot.queue.find((item) => item.id === selectedId);
  $: if (selectedId === null && snapshot.queue.length > 0) selectedId = snapshot.queue[0].id;
  $: enabledCount = snapshot.queue.filter((item) => item.enabled).length;
  $: active = snapshot.phase === "running" || snapshot.phase === "cancelling" || snapshot.phase === "probing";
  $: virtualized = snapshot.queue.length > 60;
  $: virtualStart = virtualized ? Math.max(0, Math.floor(queueScrollTop / virtualRowHeight) - virtualOverscan) : 0;
  $: virtualEnd = virtualized ? Math.min(snapshot.queue.length, virtualStart + Math.ceil(queueViewportHeight / virtualRowHeight) + virtualOverscan * 2) : snapshot.queue.length;
  $: visibleQueue = snapshot.queue.slice(virtualStart, virtualEnd);

  onMount(() => {
    let alive = true;
    bridge
      .bootstrap((event) => {
        if (!alive) return;
        if (event.event === "snapshot") snapshot = event.data;
        else if (event.event === "notice") showNotice(event.data.message, event.data.level);
        else handleMenuAction(event.data.action);
      })
      .then((value) => {
        if (!alive) return;
        snapshot = value.snapshot;
        toolchain = value.toolchain;
        version = value.version;
      })
      .catch((error) => showNotice(toMessage(error), "error"))
      .finally(() => (loading = false));

    const keydown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey;
      if (modifier && event.key.toLowerCase() === "o") {
        event.preventDefault();
        void addInputs(event.shiftKey ? "directory" : "files");
      } else if (modifier && event.key === "Enter" && snapshot.canStart) {
        event.preventDefault();
        void start();
      } else if (event.key === "Delete" && selectedId !== null && !active) {
        event.preventDefault();
        void removeSelected();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => {
      alive = false;
      window.removeEventListener("keydown", keydown);
    };
  });

  function toMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function showNotice(message: string, level = "info") {
    notice = message;
    noticeLevel = level;
  }

  function handleMenuAction(action: string) {
    if (action === "add-files" && !active) void addInputs("files");
    else if (action === "add-directory" && !active) void addInputs("directory");
    else if (action === "start-batch" && snapshot.canStart) void start();
    else if (action === "cancel-batch" && snapshot.phase === "running") void cancel();
    else if (action === "remove-selected" && !active) void removeSelected();
  }

  async function runSnapshot(action: () => Promise<SessionSnapshot>) {
    try {
      snapshot = await action();
    } catch (error) {
      showNotice(toMessage(error), "error");
    }
  }

  async function addInputs(kind: "files" | "directory") {
    await runSnapshot(() => bridge.pickInputs(kind));
  }

  async function removeSelected() {
    if (selectedId === null) return;
    const removed = selectedId;
    await runSnapshot(() => bridge.removeItems([removed]));
    selectedId = snapshot.queue.find((item) => item.id !== removed)?.id ?? null;
  }

  async function setEnabled(item: QueueItem, enabled: boolean) {
    await runSnapshot(() => bridge.setItemEnabled(item.id, enabled));
  }

  async function saveSettings() {
    await runSnapshot(() => bridge.updateSettings({ ...snapshot.settings }));
  }

  async function start() {
    try {
      await bridge.startBatch();
    } catch (error) {
      showNotice(toMessage(error), "error");
    }
  }

  async function cancel() {
    try {
      await bridge.cancelBatch();
    } catch (error) {
      showNotice(toMessage(error), "error");
    }
  }

  async function retry(item: QueueItem) {
    await runSnapshot(() => bridge.retryItems([item.id]));
  }

  async function chooseFfmpeg() {
    try {
      toolchain = await bridge.chooseFfmpeg();
    } catch (error) {
      if (toMessage(error) !== "FFmpeg selection was cancelled") showNotice(toMessage(error), "error");
    }
  }

  async function retryToolchain() {
    try {
      toolchain = await bridge.retryToolchain();
    } catch (error) {
      showNotice(toMessage(error), "error");
    }
  }

  function percent(value: number | null): string {
    return value === null ? "—" : `${Math.round(value / 10)}%`;
  }

  function eta(value: number | null): string {
    if (value === null) return "Estimating…";
    const minutes = Math.floor(value / 60);
    const seconds = value % 60;
    return minutes ? `${minutes}m ${seconds}s left` : `${seconds}s left`;
  }

  function statusLabel(status: string): string {
    return ({ ready: "Ready", compatible: "Compatible", probing: "Inspecting", queued: "Queued", preparing: "Preparing", running: "Converting", succeeded: "Done", skipped: "Skipped", planned: "Planned", failed: "Needs attention", cancelled: "Cancelled" } as Record<string, string>)[status] ?? status;
  }

  function trackQueueViewport(event: Event) {
    const element = event.currentTarget as HTMLElement;
    queueScrollTop = element.scrollTop;
    queueViewportHeight = element.clientHeight;
  }
</script>

<svelte:head><title>SonicMux</title></svelte:head>

<div class="app-shell" class:is-loading={loading}>
  <header class="topbar">
    <div class="brand" aria-label="SonicMux">
      <div class="brand-mark" aria-hidden="true"><span></span><span></span><span></span><span></span><span></span></div>
      <div><strong>SonicMux</strong><small>MKV audio compatibility</small></div>
    </div>
    <div class="top-meta">
      <div class="profile-pill"><Gauge size={15} strokeWidth={1.8} /><span>{snapshot.settings.profile}</span></div>
      <div class:unavailable={!toolchain.available} class="tool-pill" title={toolchain.detail}>
        <span class="status-dot"></span><span>{toolchain.available ? `FFmpeg · ${toolchain.source}` : "FFmpeg missing"}</span>
      </div>
    </div>
  </header>

  {#if notice}
    <div class:error={noticeLevel === "error"} class="notice" role="alert">
      <AlertTriangle size={17} /><span>{notice}</span>
      <button class="icon-button" aria-label="Dismiss notification" on:click={() => (notice = "")}><X size={16} /></button>
    </div>
  {/if}

  {#if snapshot.phase === "toolchain-setup"}
    <main class="setup-view">
      <div class="setup-card">
        <div class="setup-icon"><Wrench size={28} /></div>
        <p class="eyebrow">One-time setup</p>
        <h1>Connect FFmpeg to SonicMux</h1>
        <p>SonicMux needs an existing FFmpeg and FFprobe pair. Nothing will be downloaded automatically.</p>
        <div class="setup-detail"><AlertTriangle size={17} /><span>{toolchain.detail}</span></div>
        <div class="setup-actions">
          <button class="primary" on:click={chooseFfmpeg}><FolderOpen size={17} />Choose FFmpeg</button>
          <button class="secondary" on:click={retryToolchain}><RefreshCw size={16} />Retry system search</button>
        </div>
        <small>Choose the <code>ffmpeg{navigator.userAgent.includes("Windows") ? ".exe" : ""}</code> file next to FFprobe.</small>
      </div>
    </main>
  {:else}
    <main class="workspace">
      <section class="queue-pane" aria-labelledby="queue-heading">
        <div class="section-head">
          <div><p class="eyebrow">Batch</p><h1 id="queue-heading">Conversion queue <span>{snapshot.queue.length}</span></h1></div>
          <div class="head-actions">
            <button class="secondary compact" disabled={active} on:click={() => addInputs("directory")}><FolderOpen size={16} />Folder</button>
            <button class="primary compact" disabled={active} on:click={() => addInputs("files")}><FilePlus2 size={16} />Add MKV</button>
          </div>
        </div>

        {#if snapshot.queue.length === 0}
          <div class="empty-state">
            <div class="empty-graphic" aria-hidden="true"><Film size={38} /><div class="wave-line"></div></div>
            <h2>Drop MKV files here</h2>
            <p>Add individual movies or scan one folder. Your originals are never overwritten.</p>
            <div class="empty-actions">
              <button class="primary" on:click={() => addInputs("files")}><FilePlus2 size={17} />Choose MKV files</button>
              <button class="secondary" on:click={() => addInputs("directory")}><FolderOpen size={17} />Choose folder</button>
            </div>
            <small>MKV only in the current stable core · Ctrl/⌘ O</small>
          </div>
        {:else}
          <div class:virtualized class="queue-list" aria-label="Files in conversion queue" on:scroll={trackQueueViewport}>
            {#if virtualized && virtualStart > 0}<div class="virtual-spacer" style={`height:${virtualStart * virtualRowHeight}px`}></div>{/if}
            {#each visibleQueue as item (item.id)}
              <article class:selected={selected?.id === item.id} class:error-row={item.error} class="queue-row">
                <label class="queue-toggle" aria-label={`Include ${item.name}`}>
                  <input type="checkbox" checked={item.enabled} disabled={active} on:change={(event) => setEnabled(item, event.currentTarget.checked)} />
                  <span><Check size={12} /></span>
                </label>
                <button class="row-main" on:click={() => (selectedId = item.id)} aria-pressed={selected?.id === item.id}>
                  <span class="file-icon"><Film size={18} /></span>
                  <span class="file-copy"><strong title={item.inputDisplay}>{item.name}</strong><small>{item.plan}</small></span>
                  <span class={`state state-${item.status}`}><i></i>{statusLabel(item.status)}</span>
                  {#if item.status === "running"}
                    <span class="row-progress"><span style={`width:${item.progressMilli ? item.progressMilli / 10 : 0}%`}></span></span>
                    <span class="mono">{percent(item.progressMilli)}</span>
                  {/if}
                  <ChevronRight size={17} class="chevron" />
                </button>
                {#if item.error}
                  <div class="row-error"><AlertTriangle size={15} /><span>{item.error}</span><button on:click={() => retry(item)} disabled={active}><RotateCcw size={14} />Retry</button></div>
                {/if}
              </article>
            {/each}
            {#if virtualized && virtualEnd < snapshot.queue.length}<div class="virtual-spacer" style={`height:${(snapshot.queue.length - virtualEnd) * virtualRowHeight}px`}></div>{/if}
          </div>
        {/if}
      </section>

      <aside class="detail-pane" aria-label="File details and settings">
        <div class="tabs" role="tablist" aria-label="Detail sections">
          <button class:active={activeTab === "tracks"} role="tab" aria-selected={activeTab === "tracks"} on:click={() => (activeTab = "tracks")}><ListMusic size={15} />Tracks</button>
          <button class:active={activeTab === "settings"} role="tab" aria-selected={activeTab === "settings"} on:click={() => (activeTab = "settings")}><Settings2 size={15} />Settings</button>
          <button class:active={activeTab === "logs"} role="tab" aria-selected={activeTab === "logs"} on:click={() => (activeTab = "logs")}>Logs</button>
        </div>

        {#if activeTab === "tracks"}
          <div class="panel-content">
            {#if selected}
              <div class="inspector-title"><div><p class="eyebrow">Selected file</p><h2 title={selected.inputDisplay}>{selected.name}</h2></div>{#if !active}<button class="icon-button danger" aria-label="Remove selected file" on:click={removeSelected}><Trash2 size={17} /></button>{/if}</div>
              <p class="output-path"><HardDrive size={14} /><span title={selected.outputDisplay}>{selected.outputDisplay}</span></p>
              <div class="track-list">
                {#each selected.tracks as track}
                  <div class="track-row">
                    <span class={`track-kind track-${track.kind}`}>{track.kind === "audio" ? "A" : track.kind === "video" ? "V" : "S"}</span>
                    <span><strong>{track.title ?? `${track.kind} ${track.index}`}</strong><small>{track.codec}{track.channels ? ` · ${track.channels}ch` : ""}{track.language ? ` · ${track.language.toUpperCase()}` : ""}</small></span>
                    <span class="action-tag">{track.action}</span>
                  </div>
                {:else}
                  <div class="mini-empty"><ListMusic size={24} /><p>No track details available</p></div>
                {/each}
              </div>
            {:else}
              <div class="mini-empty"><ListMusic size={26} /><p>Select a file to inspect its tracks.</p></div>
            {/if}
          </div>
        {:else if activeTab === "settings"}
          <div class="panel-content settings-grid">
            <label>Device profile<select bind:value={snapshot.settings.profile} disabled={active} on:change={saveSettings}>{#each snapshot.profiles as profile}<option value={profile}>{profile}</option>{/each}</select></label>
            <div class="segmented" aria-label="Operation"><button class:active={snapshot.settings.action === "convert"} disabled={active} on:click={() => { snapshot.settings.action = "convert"; void saveSettings(); }}>Convert</button><button class:active={snapshot.settings.action === "remux"} disabled={active} on:click={() => { snapshot.settings.action = "remux"; void saveSettings(); }}>Remux</button></div>
            <div class="field-pair">
              <label>Codec<select bind:value={snapshot.settings.codec} disabled={active} on:change={saveSettings}><option value="ac3">AC-3</option><option value="eac3">E-AC-3</option><option value="aac">AAC</option></select></label>
              <label>Bitrate<input bind:value={snapshot.settings.bitrate} disabled={active} on:change={saveSettings} /></label>
            </div>
            <label>Channels<select bind:value={snapshot.settings.channels} disabled={active} on:change={saveSettings}><option value="keep-up-to-5.1">Keep up to 5.1</option><option value="stereo">Stereo</option><option value="5.1">Force 5.1</option></select></label>
            <label>Audio output<select bind:value={snapshot.settings.mode} disabled={active} on:change={saveSettings}><option value="add">Add compatible track</option><option value="replace">Replace source track</option><option value="only-new">Keep only new track</option></select></label>
            <label>Parallel files<input type="number" min="1" max="64" bind:value={snapshot.settings.jobs} disabled={active} on:change={saveSettings} /></label>
            <div class="output-choice"><span>Output folder<small>{snapshot.settings.outputDirectory ?? "Next to each source"}</small></span><button class="secondary compact" disabled={active} on:click={() => runSnapshot(() => bridge.pickOutputDirectory())}>Change</button></div>
            <label class="check-line"><input type="checkbox" bind:checked={snapshot.settings.dryRun} disabled={active} on:change={saveSettings} /><span>Plan only — do not create files</span></label>
          </div>
        {:else}
          <div class="panel-content logs" aria-label="Session log">{#each snapshot.logs as line}<p><span></span>{line}</p>{:else}<div class="mini-empty"><p>No session messages yet.</p></div>{/each}</div>
        {/if}
      </aside>
    </main>

    <footer class="actionbar">
      <div class="batch-summary">
        {#if active}
          <div class="aggregate-progress" aria-label={`Batch progress ${percent(snapshot.progressMilli)}`}><span style={`width:${snapshot.progressMilli ? snapshot.progressMilli / 10 : 0}%`}></span></div>
          <div><strong class="mono">{percent(snapshot.progressMilli)}</strong><small><Clock3 size={13} />{snapshot.phase === "cancelling" ? "Cleaning up safely…" : eta(snapshot.etaSeconds)}</small></div>
        {:else}
          <div class="summary-icon"><Check size={17} /></div><div><strong>{enabledCount} of {snapshot.queue.length} selected</strong><small>Video, subtitles and metadata stay untouched</small></div>
        {/if}
      </div>
      {#if snapshot.phase === "running" || snapshot.phase === "cancelling"}
        <button class="stop-button" disabled={snapshot.phase === "cancelling"} on:click={cancel}><CircleStop size={18} />{snapshot.phase === "cancelling" ? "Cancelling…" : "Cancel batch"}</button>
      {:else}
        <button class="start-button" disabled={!snapshot.canStart} on:click={start}><Play size={18} fill="currentColor" />Start conversion</button>
      {/if}
    </footer>
  {/if}

  <div class="sr-only" aria-live="polite" aria-atomic="true">{notice}{active ? `Batch ${percent(snapshot.progressMilli)}` : ""}</div>
  {#if version}<span class="version" aria-hidden="true">v{version}</span>{/if}
</div>
