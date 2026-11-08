// MediaCraft interface. Titles and other site data are only ever inserted as
// text nodes, never as HTML, so a crafted title can't run code.

type Mode = "video" | "audio";
type Status = "queued" | "running" | "paused" | "done" | "failed" | "canceled" | "interrupted";

interface Settings {
  folder: string;
  mode: Mode;
  max_height: number | null;
  container: string;
  audio_format: string;
  audio_bitrate: number | null;
  concurrency: number;
  playlist_subfolder: boolean;
  number_files: boolean;
}
interface Spec {
  mode: Mode;
  max_height: number | null;
  container: string;
  audio_format: string;
  audio_bitrate: number | null;
  folder: string;
  subfolder: string | null;
  index: number | null;
}
interface Job {
  id: number;
  url: string;
  title: string;
  thumbnail: string | null;
  playlist: string | null;
  spec: Spec;
  status: Status;
  phase: string;
  fraction: number | null;
  downloaded: number | null;
  total: number | null;
  speed: number | null;
  eta: number | null;
  file: string | null;
  error: string | null;
}
interface HeightOption { height: number; fps: number | null; size_estimate: number | null }
interface Entry { url: string; title: string; duration: number | null; thumbnail: string | null }
interface Probe {
  kind: "video" | "playlist";
  url: string;
  title: string;
  uploader: string | null;
  duration: number | null;
  thumbnail: string | null;
  heights: HeightOption[];
  audio_size_estimate: number | null;
  entries: Entry[];
  skipped_entries: number;
  also_single_video: boolean;
}
interface FolderCheck { ok: boolean; message: string; free_bytes: number | null }
interface ToolInfo {
  id: string;
  label: string;
  purpose: string;
  found: boolean;
  path: string | null;
  version: string | null;
  managed: boolean;
  outdated: boolean;
  can_install: boolean;
  download_note: string;
}
interface ToolProgress { id: string; downloaded: number; total: number | null }

declare global {
  interface Window {
    __TAURI__: {
      core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
      event: { listen<T>(name: string, cb: (e: { payload: T }) => void): Promise<() => void> };
    };
  }
}

const UNTITLED = "Unavailable or untitled video"; // Matches probe.rs.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<HTMLElementTagNameMap[K]> & { class?: string } = {},
  ...children: (Node | string | null)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  const { class: cls, ...rest } = props as { class?: string };
  if (cls) node.className = cls;
  Object.assign(node, rest);
  for (const c of children) if (c !== null) node.append(c);
  return node;
}

function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

function clock(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "";
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

function thumb(url: string | null): HTMLElement {
  const img = el("img", { class: "thumb", alt: "", loading: "lazy", referrerPolicy: "no-referrer" });
  if (url && url.startsWith("https://")) img.src = url;
  img.onerror = () => img.removeAttribute("src");
  return img;
}

const banner = $("banner");
let bannerTimer = 0;
function showBanner(message: string, kind: "error" | "warn" = "error") {
  banner.textContent = message;
  banner.className = kind === "warn" ? "banner warn" : "banner";
  banner.hidden = false;
  window.clearTimeout(bannerTimer);
  bannerTimer = window.setTimeout(() => (banner.hidden = true), 9000);
}

// ---------- settings ----------

let settings: Settings;
let folderState: FolderCheck = { ok: false, message: "", free_bytes: null };

function saveSettings() {
  invoke("save_settings", { settings }).catch((e) => showBanner(String(e)));
}

async function refreshFolder() {
  folderState = await invoke<FolderCheck>("check_folder", { path: settings.folder });
  renderFolderNote();
}

// ---------- analyzer ----------

const analyzer = $("analyzer");
let probe: Probe | null = null;
let selected = new Set<number>();
let analyzing = 0;
let folderNote: HTMLElement | null = null;
let downloadBtn: HTMLButtonElement | null = null;

function paintFolderNote(note: HTMLElement) {
  if (!folderState.ok) {
    note.className = "folder-note bad";
    note.textContent = folderState.message;
  } else {
    note.className = "folder-note ok";
    const free = folderState.free_bytes !== null ? `${bytes(folderState.free_bytes)} free` : "";
    note.textContent = [folderState.message, free].filter(Boolean).join(" · ");
  }
}

function renderFolderNote() {
  if (folderNote) paintFolderNote(folderNote);
  updateDownloadButton();
}

async function pickFolder(): Promise<boolean> {
  const picked = await invoke<string | null>("pick_folder");
  if (!picked) return false;
  settings.folder = picked;
  saveSettings();
  return true;
}

function updateDownloadButton() {
  if (!downloadBtn || !probe) return;
  const count = probe.kind === "playlist" ? selected.size : 1;
  downloadBtn.disabled = !folderState.ok || count === 0;
  downloadBtn.textContent = probe.kind === "playlist" ? `Download ${count} of ${probe.entries.length}` : "Download";
}

function select(
  label: string,
  options: [string, string][],
  value: string,
  onChange: (v: string) => void,
): HTMLElement {
  const s = el("select", { ariaLabel: label });
  for (const [v, t] of options) s.append(el("option", { value: v, textContent: t }));
  s.value = options.some(([v]) => v === value) ? value : options[0][0];
  s.onchange = () => onChange(s.value);
  return el("label", { class: "field" }, label, s);
}

function qualityOptions(p: Probe): [string, string][] {
  const best: [string, string] = ["", "Best available"];
  if (p.kind === "video" && p.heights.length > 0) {
    return [
      best,
      ...p.heights.map((h): [string, string] => {
        const fps = h.fps && h.fps > 30 ? ` ${Math.round(h.fps)} fps` : "";
        const size = h.size_estimate ? ` · ~${bytes(h.size_estimate)}` : "";
        return [String(h.height), `${h.height}p${fps}${size}`];
      }),
    ];
  }
  return [best, ...[2160, 1440, 1080, 720, 480, 360].map((h): [string, string] => [String(h), `Up to ${h}p`])];
}

function renderOptions(p: Probe): HTMLElement {
  const box = el("div", { class: "options" });
  const seg = el("div", { class: "segmented", role: "group", ariaLabel: "Download type" });
  (["video", "audio"] as Mode[]).forEach((m) => {
    const b = el("button", { type: "button", textContent: m === "video" ? "Video" : "Audio only" });
    b.setAttribute("aria-pressed", String(settings.mode === m));
    b.onclick = () => {
      settings.mode = m;
      saveSettings();
      renderAnalyzer();
    };
    seg.append(b);
  });
  box.append(el("div", { class: "field" }, "Type", seg));

  if (settings.mode === "video") {
    // Show what will really happen: the closest quality at or below the preference.
    const options = qualityOptions(p);
    const cap = settings.max_height;
    const shown = cap === null
      ? ""
      : (options.map(([v]) => Number(v)).filter((h) => h > 0 && h <= cap).sort((a, b) => b - a)[0] ?? "");
    box.append(
      select("Quality", options, String(shown), (v) => {
        settings.max_height = v ? Number(v) : null;
        saveSettings();
      }),
      select("Format", [["mp4", "MP4"], ["mkv", "MKV"], ["webm", "WebM"]], settings.container, (v) => {
        settings.container = v;
        saveSettings();
      }),
    );
  } else {
    const lossless = settings.audio_format === "flac" || settings.audio_format === "wav";
    box.append(
      select(
        "Format",
        [["mp3", "MP3"], ["m4a", "M4A"], ["opus", "Opus"], ["flac", "FLAC"], ["wav", "WAV"]],
        settings.audio_format,
        (v) => {
          settings.audio_format = v;
          saveSettings();
          renderAnalyzer();
        },
      ),
    );
    if (!lossless) {
      box.append(
        select(
          "Quality",
          [["", "Best available"], ["320", "320 kbps"], ["192", "192 kbps"], ["128", "128 kbps"]],
          String(settings.audio_bitrate ?? ""),
          (v) => {
            settings.audio_bitrate = v ? Number(v) : null;
            saveSettings();
          },
        ),
      );
    }
    if (p.audio_size_estimate) {
      box.append(el("div", { class: "field" }, "Source audio size", `~${bytes(p.audio_size_estimate)}`));
    }
  }
  return box;
}

function renderFolderRow(): HTMLElement {
  folderNote = el("span", { class: "folder-note" });
  const change = el("button", { type: "button", class: "ghost small", textContent: "Change" });
  change.onclick = async () => {
    if (await pickFolder()) {
      renderAnalyzer();
      await refreshFolder();
    }
  };
  const path = el("div", { class: "folder-path", dir: "auto" }, `Save to: ${settings.folder || "No folder chosen"}`);
  return el("div", {}, el("div", { class: "folder-row" }, path, change), folderNote);
}

function renderPlaylist(p: Probe): HTMLElement {
  const wrap = el("div", {});
  const toggles = new Map<number, HTMLInputElement>();
  const list = el("ul", { class: "entries" });
  p.entries.forEach((e, i) => {
    const cb = el("input", { type: "checkbox", checked: selected.has(i), ariaLabel: e.title });
    cb.onchange = () => {
      if (cb.checked) selected.add(i);
      else selected.delete(i);
      updateDownloadButton();
    };
    toggles.set(i, cb);
    list.append(
      el(
        "li",
        {},
        cb,
        el("span", { class: "e-num", textContent: String(i + 1) }),
        el("span", { class: "e-title", dir: "auto", title: e.title, textContent: e.title }),
        el("span", { class: "e-dur", textContent: clock(e.duration) }),
      ),
    );
  });
  const setAll = (on: boolean) => {
    selected = new Set(on ? p.entries.map((_, i) => i) : []);
    toggles.forEach((cb, i) => (cb.checked = selected.has(i)));
    updateDownloadButton();
  };
  const all = el("button", { type: "button", class: "ghost small", textContent: "Select all" });
  all.onclick = () => setAll(true);
  const none = el("button", { type: "button", class: "ghost small", textContent: "Select none" });
  none.onclick = () => setAll(false);
  const tools = el("div", { class: "entry-tools" }, all, none);
  if (p.skipped_entries > 0) {
    tools.append(el("span", { class: "muted", textContent: `${p.skipped_entries} unavailable item(s) left out` }));
  }
  wrap.append(tools, list);
  return wrap;
}

function checkbox(label: string, value: boolean, onChange: (v: boolean) => void): HTMLElement {
  const cb = el("input", { type: "checkbox", checked: value });
  cb.onchange = () => onChange(cb.checked);
  return el("label", { class: "check" }, cb, label);
}

function renderAnalyzer() {
  analyzer.replaceChildren();
  if (!probe) {
    analyzer.hidden = true;
    return;
  }
  analyzer.hidden = false;
  const p = probe;
  const meta = el("div", { class: "media-meta" });
  meta.append(el("div", { class: "media-title", dir: "auto", textContent: p.title }));
  const facts: string[] = [];
  if (p.uploader) facts.push(p.uploader);
  if (p.kind === "video" && p.duration) facts.push(clock(p.duration));
  if (p.kind === "playlist") facts.push(`Playlist · ${p.entries.length} videos`);
  meta.append(el("div", { class: "muted", dir: "auto", textContent: facts.join(" · ") }));
  if (p.also_single_video) {
    const only = el("button", { type: "button", class: "ghost small", textContent: "Only the video in this link" });
    only.onclick = () => analyze(p.url, true);
    meta.append(el("div", { class: "scope-switch" }, only));
  }
  analyzer.append(el("div", { class: "media-head" }, thumb(p.thumbnail), meta));
  analyzer.append(renderOptions(p));
  if (p.kind === "playlist") analyzer.append(renderPlaylist(p));
  analyzer.append(renderFolderRow());

  const left = el("div", {});
  if (p.kind === "playlist") {
    left.append(
      checkbox("Save in a folder named after the playlist", settings.playlist_subfolder, (v) => {
        settings.playlist_subfolder = v;
        saveSettings();
      }),
      checkbox("Number the files", settings.number_files, (v) => {
        settings.number_files = v;
        saveSettings();
      }),
    );
  }
  downloadBtn = el("button", { type: "button", class: "primary", textContent: "Download" });
  downloadBtn.onclick = startDownload;
  analyzer.append(el("div", { class: "actions" }, left, downloadBtn));
  renderFolderNote();
}

function specFor(index: number | null, subfolder: string | null): Spec {
  return {
    mode: settings.mode,
    max_height: settings.max_height,
    container: settings.container,
    audio_format: settings.audio_format,
    audio_bitrate: settings.audio_bitrate,
    folder: settings.folder,
    subfolder,
    index,
  };
}

async function startDownload() {
  if (!probe) return;
  const p = probe;
  let requests;
  if (p.kind === "playlist") {
    const sub = settings.playlist_subfolder ? p.title : null;
    requests = [...selected].sort((a, b) => a - b).map((i) => ({
      url: p.entries[i].url,
      title: p.entries[i].title,
      thumbnail: p.entries[i].thumbnail,
      playlist: p.title,
      spec: specFor(settings.number_files ? i + 1 : null, sub),
    }));
  } else {
    requests = [{ url: p.url, title: p.title, thumbnail: p.thumbnail, playlist: null, spec: specFor(null, null) }];
  }
  try {
    await invoke("enqueue", { requests });
    probe = null;
    ($("url") as HTMLInputElement).value = "";
    renderAnalyzer();
  } catch (e) {
    showBanner(String(e));
    await refreshFolder();
  }
}

async function analyze(url: string, noPlaylist: boolean) {
  const run = ++analyzing;
  const btn = $<HTMLButtonElement>("analyze");
  btn.disabled = true;
  banner.hidden = true;
  probe = null;
  analyzer.hidden = false;
  analyzer.replaceChildren(el("div", { class: "loading", textContent: "Reading the link…" }));
  try {
    const result = await invoke<Probe>("probe_url", { url, noPlaylist });
    if (run !== analyzing) return;
    probe = result;
    selected = new Set(result.entries.flatMap((e, i) => (e.title === UNTITLED ? [] : [i])));
    renderAnalyzer();
    await refreshFolder();
  } catch (e) {
    if (run !== analyzing) return;
    analyzer.hidden = true;
    showBanner(String(e));
  } finally {
    if (run === analyzing) btn.disabled = false;
  }
}

// ---------- queue ----------

const jobsList = $("jobs");
const jobs = new Map<number, Job>();
const rows = new Map<number, HTMLElement>();

function jobLine(j: Job): { text: string; cls: string } {
  switch (j.status) {
    case "queued":
      return { text: "Waiting", cls: "" };
    case "running": {
      const parts = [j.phase];
      if (j.fraction !== null) parts[0] += ` ${Math.round(j.fraction * 100)}%`;
      if (j.total) parts.push(`${bytes(j.downloaded)} of ${bytes(j.total)}`);
      if (j.speed) parts.push(`${bytes(j.speed)}/s`);
      if (j.eta) parts.push(`${clock(j.eta)} left`);
      return { text: parts.join(" · "), cls: "" };
    }
    case "paused": {
      const parts = [j.fraction !== null ? `Paused at ${Math.round(j.fraction * 100)}%` : "Paused"];
      if (j.total) parts.push(`${bytes(j.downloaded)} of ${bytes(j.total)}`);
      return { text: parts.join(" · "), cls: "" };
    }
    case "done":
      return { text: `Done${j.total ? ` · ${bytes(j.total)}` : ""}`, cls: "good" };
    case "failed":
      return { text: j.error ?? "Failed", cls: "bad" };
    case "canceled":
      return { text: "Canceled. Retry continues from where it stopped.", cls: "" };
    case "interrupted":
      return { text: "Interrupted when MediaCraft closed. Retry to continue.", cls: "" };
  }
}

function actionButton(label: string, handler: () => unknown, cls = "link"): HTMLButtonElement {
  const b = el("button", { type: "button", class: cls, textContent: label });
  b.onclick = async () => {
    try {
      await handler();
    } catch (e) {
      showBanner(String(e));
    }
  };
  return b;
}

function renderJob(j: Job): HTMLElement {
  const line = jobLine(j);
  const body = el(
    "div",
    { class: "job-body" },
    el("div", { class: "job-title", dir: "auto", title: j.title, textContent: j.title }),
    el("div", { class: `job-sub ${line.cls}`, textContent: line.text }),
  );
  if (j.status === "running" || (j.status === "paused" && j.fraction !== null)) {
    const bar = el("div", { class: j.status === "paused" ? "bar paused" : j.fraction === null ? "bar busy" : "bar" });
    const fill = el("i", {});
    if (j.fraction !== null) fill.style.width = `${(j.fraction * 100).toFixed(1)}%`;
    bar.append(fill);
    body.append(bar);
  }
  const actions = el("div", { class: "job-actions" });
  const id = j.id;
  if (j.status === "running" || j.status === "queued") {
    actions.append(actionButton("Pause", () => invoke("pause_job", { id })));
  }
  if (j.status === "paused") {
    actions.append(actionButton("Resume", () => invoke("retry_job", { id })));
  }
  if (j.status === "running" || j.status === "queued" || j.status === "paused") {
    actions.append(actionButton("Cancel", () => invoke("cancel_job", { id }), "link danger"));
  }
  if (j.status === "failed" || j.status === "canceled" || j.status === "interrupted") {
    actions.append(actionButton("Retry", () => invoke("retry_job", { id })));
  }
  if (j.status === "done") {
    actions.append(
      actionButton("Open", () => invoke("open_job", { id, reveal: false })),
      actionButton("Show in folder", () => invoke("open_job", { id, reveal: true })),
    );
  }
  if (j.status !== "running" && j.status !== "queued") {
    actions.append(actionButton("Remove", () => invoke("remove_job", { id })));
  }
  return el("li", { class: "job" }, thumb(j.thumbnail), body, actions);
}

function queueSummary() {
  const all = [...jobs.values()];
  const active = all.filter((j) => j.status === "running").length;
  const waiting = all.filter((j) => j.status === "queued").length;
  const paused = all.filter((j) => j.status === "paused").length;
  const resumable = paused + all.filter((j) => j.status === "interrupted").length;
  const bits: string[] = [];
  if (active) bits.push(`${active} active`);
  if (waiting) bits.push(`${waiting} waiting`);
  if (paused) bits.push(`${paused} paused`);
  $("queue-count").textContent = bits.length ? `· ${bits.join(", ")}` : all.length ? `· ${all.length}` : "";
  $("empty").hidden = all.length > 0;
  $("pause-all").hidden = active + waiting === 0;
  $("resume-all").hidden = resumable === 0;
  $("cancel-all").hidden = active + waiting + paused === 0;
}

function upsertJob(j: Job) {
  const old = rows.get(j.id);
  const prevStatus = jobs.get(j.id)?.status;
  jobs.set(j.id, j);
  const row = renderJob(j);
  if (old) old.replaceWith(row);
  else jobsList.prepend(row);
  rows.set(j.id, row);
  if (prevStatus !== j.status) queueSummary();
}

function removeJob(id: number) {
  rows.get(id)?.remove();
  rows.delete(id);
  jobs.delete(id);
  queueSummary();
}

// ---------- components ----------

const dialog = $<HTMLDialogElement>("components");
const toolProgress = new Map<string, ToolProgress>();
const installing = new Set<string>();
let toolsCache: ToolInfo[] = [];

function renderTools() {
  const list = $("tools-list");
  list.replaceChildren();
  for (const t of toolsCache) {
    const state = !t.found
      ? el("span", { class: "tool-state bad", textContent: "Not installed" })
      : t.outdated
        ? el("span", { class: "tool-state warn", textContent: `Version ${t.version} is out of date` })
        : el("span", { class: "tool-state good", textContent: t.version ? `Ready · ${t.version}` : "Ready" });
    const info = el(
      "div",
      { class: "tool-info" },
      el("div", { class: "tool-name", textContent: t.label }),
      el("div", { class: "tool-detail", textContent: t.purpose }),
      el("div", { class: "tool-detail" }, state),
    );
    if (t.path) info.append(el("div", { class: "tool-detail", textContent: t.path }));
    else info.append(el("div", { class: "tool-detail", textContent: t.download_note }));

    const row = el("div", { class: "tool" }, info);
    const prog = toolProgress.get(t.id);
    if (installing.has(t.id)) {
      const text = prog
        ? prog.total
          ? `${Math.round((prog.downloaded / prog.total) * 100)}% of ${bytes(prog.total)}`
          : bytes(prog.downloaded)
        : "Starting…";
      row.append(el("span", { class: "muted", textContent: text }));
    } else if (t.can_install && (!t.found || t.outdated || (t.id === "yt-dlp" && t.managed))) {
      const label = !t.found ? "Install" : "Update";
      const b = el("button", { type: "button", class: "primary", textContent: label, title: t.download_note });
      b.onclick = () => installTool(t.id);
      row.append(b);
    }
    list.append(row);
  }
}

async function refreshTools() {
  toolsCache = await invoke<ToolInfo[]>("tools_status");
  const needsAttention = toolsCache.some((t) => !t.found || t.outdated);
  $("components-dot").hidden = !needsAttention;
  renderTools();
  return toolsCache;
}

async function installTool(id: string) {
  installing.add(id);
  renderTools();
  try {
    await invoke("install_tool", { id });
    banner.hidden = true; // Any warning about this tool is now out of date.
  } catch (e) {
    showBanner(String(e));
  }
  installing.delete(id);
  toolProgress.delete(id);
  await refreshTools();
}

// ---------- settings window ----------

const settingsDialog = $<HTMLDialogElement>("settings");

function settingsSection(title: string, ...children: HTMLElement[]): HTMLElement {
  return el("section", { class: "settings-section" }, el("h3", { textContent: title }), ...children);
}

function renderSettings() {
  const row = (...children: HTMLElement[]) => el("div", { class: "settings-row" }, ...children);
  const change = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    settings[key] = value;
    saveSettings();
  };

  const note = el("span", { class: "folder-note" });
  paintFolderNote(note);
  const pick = el("button", { type: "button", class: "ghost small", textContent: "Change" });
  pick.onclick = async () => {
    if (await pickFolder()) {
      await refreshFolder();
      renderSettings();
    }
  };
  const folder = el(
    "div",
    {},
    el(
      "div",
      { class: "folder-row" },
      el("div", { class: "folder-path", dir: "auto", textContent: settings.folder || "No folder chosen" }),
      pick,
    ),
    note,
  );

  const lossless = settings.audio_format === "flac" || settings.audio_format === "wav";
  const audio = row(
    select(
      "Format",
      [["mp3", "MP3"], ["m4a", "M4A"], ["opus", "Opus"], ["flac", "FLAC"], ["wav", "WAV"]],
      settings.audio_format,
      (v) => {
        change("audio_format", v);
        renderSettings();
      },
    ),
  );
  if (!lossless) {
    audio.append(
      select(
        "Quality",
        [["", "Best available"], ["320", "320 kbps"], ["192", "192 kbps"], ["128", "128 kbps"]],
        String(settings.audio_bitrate ?? ""),
        (v) => change("audio_bitrate", v ? Number(v) : null),
      ),
    );
  }

  $("settings-body").replaceChildren(
    settingsSection("Save folder", folder),
    settingsSection(
      "Downloads",
      row(
        select("Default type", [["video", "Video"], ["audio", "Audio only"]], settings.mode, (v) =>
          change("mode", v as Mode),
        ),
        select(
          "At once",
          ["1", "2", "3", "4", "6", "8"].map((n): [string, string] => [n, n]),
          String(settings.concurrency),
          (v) => change("concurrency", Number(v)),
        ),
      ),
    ),
    settingsSection(
      "Video",
      row(
        select(
          "Quality",
          [
            ["", "Best available"],
            ...[2160, 1440, 1080, 720, 480, 360].map((h): [string, string] => [String(h), `Up to ${h}p`]),
          ],
          String(settings.max_height ?? ""),
          (v) => change("max_height", v ? Number(v) : null),
        ),
        select("Format", [["mp4", "MP4"], ["mkv", "MKV"], ["webm", "WebM"]], settings.container, (v) =>
          change("container", v),
        ),
      ),
    ),
    settingsSection("Audio", audio),
    settingsSection(
      "Playlists",
      row(
        checkbox("Save in a folder named after the playlist", settings.playlist_subfolder, (v) =>
          change("playlist_subfolder", v),
        ),
        checkbox("Number the files", settings.number_files, (v) => change("number_files", v)),
      ),
    ),
  );
}

// ---------- about window ----------

const aboutDialog = $<HTMLDialogElement>("about");
const LOGO = ["X...X", "XX.XX", "X.X.X", "X...X", "X...X"];
const BLOCK_COLORS = ["--accent", "--good", "--warn", "--bad"];

function buildLogo() {
  const logo = $("about-logo");
  let placed = 0;
  for (const line of LOGO) {
    for (const cell of line) {
      const block = el("i", {});
      if (cell === "X") {
        block.style.background = `var(${BLOCK_COLORS[placed % BLOCK_COLORS.length]})`;
        block.style.animationDelay = `${placed * 40}ms`;
        placed++;
      }
      logo.append(block);
    }
  }
  aboutDialog.querySelectorAll<HTMLElement>(".about-line").forEach((line, i) => {
    line.style.animationDelay = `${950 + i * 70}ms`;
  });
}

function openAbout() {
  aboutDialog.classList.remove("play");
  void aboutDialog.offsetWidth; // Restarts the animation on every open.
  aboutDialog.classList.add("play");
  aboutDialog.showModal();
}

// ---------- start up ----------

async function start() {
  const boot = await invoke<{ settings: Settings; jobs: Job[]; version: string }>("bootstrap");
  settings = boot.settings;
  $("about-version").textContent = `· v${boot.version}`;
  buildLogo();
  boot.jobs.forEach(upsertJob);
  queueSummary();

  const conc = $<HTMLSelectElement>("concurrency");
  conc.value = String(settings.concurrency);
  conc.onchange = () => {
    settings.concurrency = Number(conc.value);
    saveSettings();
  };

  await listen<Job>("job-updated", (e) => upsertJob(e.payload));
  await listen<number>("job-removed", (e) => removeJob(e.payload));
  await listen<ToolProgress>("tool-progress", (e) => {
    toolProgress.set(e.payload.id, e.payload);
    renderTools();
  });

  const urlInput = $<HTMLInputElement>("url");
  $<HTMLFormElement>("url-form").onsubmit = (ev) => {
    ev.preventDefault();
    const url = urlInput.value.trim();
    if (url) analyze(url, false);
    else urlInput.focus();
  };
  $("paste").onclick = async () => {
    try {
      const text = (await navigator.clipboard.readText()).trim();
      if (text) {
        urlInput.value = text;
        analyze(text, false);
      }
    } catch {
      urlInput.focus();
      showBanner("Couldn't read the clipboard. Press Ctrl+V in the box instead.", "warn");
    }
  };
  $("clear").onclick = () => invoke("clear_finished");
  $("pause-all").onclick = () => invoke("pause_all");
  $("resume-all").onclick = () => invoke("resume_all").catch((e) => showBanner(String(e)));
  $("cancel-all").onclick = () => invoke("cancel_all");
  $("open-components").onclick = () => {
    dialog.showModal();
    refreshTools();
  };
  $("close-components").onclick = () => dialog.close();

  $("open-settings").onclick = () => {
    renderSettings();
    settingsDialog.showModal();
  };
  $("close-settings").onclick = () => settingsDialog.close();
  // Also runs when Esc closes the window.
  settingsDialog.onclose = () => {
    conc.value = String(settings.concurrency);
    if (probe) renderAnalyzer();
    renderFolderNote();
  };

  $("open-about").onclick = openAbout;
  $("close-about").onclick = () => aboutDialog.close();
  $("open-source").onclick = () => invoke("open_project_page").catch((e) => showBanner(String(e)));

  await refreshFolder();
  urlInput.focus();

  const tools = await refreshTools();
  const missing = tools.filter((t) => !t.found && t.id !== "js");
  if (missing.length > 0) {
    dialog.showModal();
  } else {
    const yt = tools.find((t) => t.id === "yt-dlp");
    const js = tools.find((t) => t.id === "js");
    if (yt?.outdated) showBanner("yt-dlp is out of date and YouTube may fail. Update it in Components.", "warn");
    else if (js && !js.found) showBanner("No JavaScript runtime found, so some YouTube formats may be missing. Add one in Components.", "warn");
  }
}

start().catch((e) => showBanner(String(e)));

export {};
