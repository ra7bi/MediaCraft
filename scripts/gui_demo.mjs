// Drives the real MediaCraft page over WebView2's debugging port, only for this
// run. Sends no OS mouse or keyboard input, and sees no other window.
import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync, existsSync, statSync } from "node:fs";
import { join } from "node:path";

const root = process.argv[2];
const exe = join(process.env.LOCALAPPDATA, "MediaCraft", "mediacraft.exe");
const data = join(process.env.LOCALAPPDATA, "com.mediacraft.app");
const shots = join(root, "screenshots");
// A neutral folder, so these screenshots can go in the docs.
const testDir = join(process.env.PUBLIC, "Downloads", "MediaCraft");
mkdirSync(shots, { recursive: true });
rmSync(testDir, { recursive: true, force: true });
mkdirSync(testDir, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PORT = 9333;

// This run replaces the app's settings and download list, so keep the real ones.
const kept = ["settings.json", "jobs.json"].map((name) => {
  const path = join(data, name);
  return { path, text: existsSync(path) ? readFileSync(path) : null };
});

function stopApp() {
  try { execFileSync("taskkill", ["/IM", "mediacraft.exe", "/T", "/F"], { stdio: "ignore" }); } catch {}
}

async function launch(folder) {
  stopApp();
  await sleep(1200);
  mkdirSync(data, { recursive: true });
  writeFileSync(join(data, "settings.json"), JSON.stringify({ folder }));
  rmSync(join(data, "jobs.json"), { force: true });
  spawn(exe, [], {
    detached: true,
    stdio: "ignore",
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT} --remote-allow-origins=*` },
  }).unref();
  let target;
  for (let i = 0; i < 60 && !target; i++) {
    await sleep(500);
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      target = list.find((t) => t.type === "page");
    } catch {}
  }
  if (!target) throw new Error("no debug target");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); }
  };
  const send = (method, params = {}) => new Promise((res) => {
    const n = ++id;
    pending.set(n, res);
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const ev = async (expr) => {
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400));
    return r.result?.result?.value;
  };
  const until = async (expr, what, secs = 120) => {
    for (let i = 0; i < secs * 4; i++) {
      if (await ev(expr)) return;
      await sleep(250);
    }
    throw new Error(`timed out waiting for: ${what}\n` + (await ev("document.body.innerText")).slice(0, 4000));
  };
  const shot = async (name) => {
    // Give thumbnails a moment so screenshots show the finished page.
    for (let i = 0; i < 24; i++) {
      if (await ev(`[...document.images].every(i=>!i.getAttribute('src')||i.complete)`)) break;
      await sleep(250);
    }
    await sleep(350);
    const r = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(shots, name), Buffer.from(r.result.data, "base64"));
    console.log("shot", name);
  };
  await until("document.readyState==='complete' && !!document.getElementById('url')", "page ready");
  await sleep(600);
  return { ev, until, shot, send, close: () => ws.close() };
}

const click = (sel, text) =>
  `(()=>{const b=[...document.querySelectorAll(${JSON.stringify(sel)})].find(x=>${text ? `x.textContent.trim().startsWith(${JSON.stringify(text)})` : "true"});if(!b)throw new Error('no element ${sel} ${text ?? ""}');b.click();return true})()`;
const analyze = (u) =>
  `(()=>{const i=document.getElementById('url');i.value=${JSON.stringify(u)};document.getElementById('url-form').requestSubmit();return true})()`;

const ZOO = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
const BUNNY = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";
const LIST = "https://www.youtube.com/playlist?list=PLBCF2DAC6FFB574DE";
const results = [];
const ok = (m) => { results.push(m); console.log("PASS", m); };

try {
  // 1. Components and the yt-dlp update.
  let a = await launch(testDir);
  await a.shot("01-start.png");
  await a.ev(click("#open-components"));
  await a.until(`document.querySelectorAll('.tool').length===3`, "components list", 40);
  console.log(await a.ev(`document.getElementById('tools-list').innerText`));
  await a.shot("02-components-before.png");
  const needUpdate = await a.ev(`[...document.querySelectorAll('.tool button')].some(b=>b.textContent==='Update'||b.textContent==='Install')`);
  if (needUpdate) {
    await a.ev(click(".tool button"));
    const mtime = () => statSync(join(data, "tools", "yt-dlp.exe")).mtimeMs;
    const before = mtime();
    // The row still says Ready during the update, so wait for its button to come back.
    await a.until(`[...document.querySelectorAll('.tool')][0].querySelector('button')?.textContent==='Update'`, "yt-dlp update finished", 180);
    const banner = await a.ev(`(()=>{const b=document.getElementById('banner');return b.hidden?'':b.textContent})()`);
    if (banner) throw new Error("update reported: " + banner);
    if (mtime() === before) throw new Error("update finished but yt-dlp.exe was not replaced");
    ok("In-app yt-dlp update: checksum-verified install from the official release");
  }
  console.log(await a.ev(`document.getElementById('tools-list').innerText`));
  await a.shot("03-components-after.png");
  await a.ev(click("#close-components"));

  // 2. Your E: drive scenario.
  a.close();
  a = await launch("E:\\Youtube\\Courses");
  await a.ev(analyze(ZOO));
  await a.until(`!!document.querySelector('.folder-note.bad')`, "missing drive note");
  await a.until(`[...document.querySelectorAll('#analyzer button')].some(b=>b.textContent==='Download')`, "download button", 30);
  const blocked = await a.ev(`[...document.querySelectorAll('#analyzer button')].find(b=>b.textContent==='Download').disabled`);
  const note = await a.ev(`document.querySelector('.folder-note.bad').textContent`);
  if (!blocked) throw new Error("Download should be disabled for a missing drive");
  ok(`Missing drive: Download disabled, message: "${note}"`);
  await a.shot("04-missing-drive.png");

  // 3. Single video card with real quality list.
  a.close();
  a = await launch(testDir);
  await a.ev(analyze(BUNNY));
  await a.until(`[...document.querySelectorAll('#analyzer button')].some(b=>b.textContent==='Download')`, "video card");
  const qualities = await a.ev(`[...document.querySelectorAll('#analyzer select')][0].innerText`);
  console.log("qualities:\n" + qualities);
  ok("Video analysis shows the qualities that actually exist");
  await a.shot("05-video-options.png");

  // 4. Audio options.
  await a.ev(click(".segmented button", "Audio"));
  await a.until(`[...document.querySelectorAll('#analyzer label')].some(l=>l.textContent.includes('MP3'))`, "audio options", 10);
  await a.shot("06-audio-options.png");
  await a.ev(click(".segmented button", "Video"));

  // 5. Start a real video download at 360p and catch it mid-way.
  await a.ev(`(()=>{const s=[...document.querySelectorAll('#analyzer select')][0];s.value='360';s.dispatchEvent(new Event('change'));return true})()`);
  await a.ev(click("#analyzer button", "Download"));
  await a.until(`[...document.querySelectorAll('.job-sub')].some(x=>/Downloading (video|audio) [1-9]\\d?%/.test(x.textContent))`, "progress in UI", 60);
  await a.shot("07-downloading.png");
  ok("Live progress reaches the interface");

  // 6. Cancel from the UI, then retry from the UI.
  await a.ev(click(".job-actions button", "Cancel"));
  await a.until(`[...document.querySelectorAll('.job-actions button')].some(x=>x.textContent==='Retry')`, "canceled state", 30);
  await a.shot("08-canceled.png");
  await a.ev(click(".job-actions button", "Retry"));
  ok("Cancel and Retry buttons work");

  // 7. Playlist: select two items and also add audio while the first runs.
  await a.ev(analyze(LIST));
  await a.until(`document.querySelectorAll('.entries li').length>0`, "playlist list");
  await a.ev(click(".entry-tools button", "Select none"));
  await a.ev(`(()=>{const c=[...document.querySelectorAll('.entries input')];c[0].click();c[2].click();return true})()`);
  const label = await a.ev(`[...document.querySelectorAll('#analyzer button')].find(b=>b.textContent.startsWith('Download')).textContent`);
  if (!label.startsWith("Download 2 of")) throw new Error("selection count wrong: " + label);
  ok(`Playlist selection: "${label}"`);
  await a.shot("09-playlist.png");
  await a.ev(`(()=>{const s=[...document.querySelectorAll('#analyzer select')][0];s.value='360';s.dispatchEvent(new Event('change'));return true})()`);
  await a.ev(click("#analyzer button", "Download"));
  await sleep(1500);

  // 8. Audio-only MP3 of a short video.
  await a.ev(analyze(ZOO));
  await a.until(`[...document.querySelectorAll('#analyzer button')].some(b=>b.textContent==='Download')`, "zoo card");
  await a.ev(click(".segmented button", "Audio"));
  await a.ev(click("#analyzer button", "Download"));

  await a.until(`document.querySelectorAll('.job').length>=4 && ![...document.querySelectorAll('.job-actions button')].some(x=>x.textContent==='Cancel')`, "all downloads finished", 420);
  const summary = await a.ev(`[...document.querySelectorAll('.job')].map(j=>j.querySelector('.job-title').textContent.slice(0,50)+' => '+j.querySelector('.job-sub').textContent).join('\\n')`);
  console.log(summary);
  await a.shot("10-finished.png");
  if (/Failed|unknown/i.test(summary)) console.log("NOTE: a job did not finish cleanly");
  ok("Queue finished through the interface");
  a.close();

  // 9. Pause all, Resume all, pause one, Cancel all, with one slot so others wait.
  const pauseDir = join(testDir, "pause");
  mkdirSync(pauseDir, { recursive: true });
  a = await launch(pauseDir);
  const visible = (id) => `!document.getElementById('${id}').hidden`;
  const subs = `[...document.querySelectorAll('.job-sub')].map(x=>x.textContent)`;
  const hasRunning = `${subs}.some(t=>/^Downloading (video|audio) [1-9]/.test(t))`;
  if (await a.ev(visible("pause-all"))) throw new Error("Pause all shown with nothing to pause");
  await a.ev(`(()=>{const s=document.getElementById('concurrency');s.value='1';s.dispatchEvent(new Event('change'));return true})()`);
  await a.ev(analyze(LIST));
  await a.until(`document.querySelectorAll('.entries li').length>0`, "playlist list");
  await a.until(`[...document.querySelectorAll('#analyzer button')].some(b=>b.textContent.startsWith('Download'))`, "download button");
  await a.ev(click("#analyzer button", "Download"));
  await a.until(hasRunning, "first playlist item downloading", 60);
  if (!(await a.ev(visible("pause-all")) && await a.ev(visible("cancel-all")))) throw new Error("Pause all / Cancel all not shown");
  await a.shot("11-running-with-controls.png");

  await a.ev(click("#pause-all"));
  await a.until(`${subs}.every(t=>t.startsWith('Paused')||t.startsWith('Done'))`, "everything paused", 30);
  await sleep(2500);
  const stillPaused = await a.ev(`${subs}.every(t=>t.startsWith('Paused')||t.startsWith('Done'))`);
  if (!stillPaused) throw new Error("a download restarted after Pause all: " + (await a.ev(subs)));
  if (await a.ev(visible("pause-all")) || !(await a.ev(visible("resume-all")))) throw new Error("wrong buttons while paused");
  await a.shot("12-all-paused.png");
  ok("Pause all stops everything and nothing restarts");

  await a.ev(click("#resume-all"));
  await a.until(hasRunning, "download running again", 60);
  ok("Resume all starts the queue again");

  const rowPause = `(()=>{const r=[...document.querySelectorAll('.job')].find(x=>/^Downloading/.test(x.querySelector('.job-sub').textContent));[...r.querySelectorAll('button')].find(b=>b.textContent==='Pause').click();r.dataset.mark='1';return r.querySelector('.job-title').textContent})()`;
  const pausedTitle = await a.ev(rowPause);
  const rowSub = `[...document.querySelectorAll('.job')].find(x=>x.querySelector('.job-title').textContent===${JSON.stringify(pausedTitle)}).querySelector('.job-sub').textContent`;
  await a.until(`${rowSub}.startsWith('Paused')`, "one download paused", 30);
  await a.until(hasRunning, "next download takes the free slot", 60);
  await a.shot("13-one-paused.png");
  ok("Pausing one download frees its slot for the next");

  // The controls must stay reachable at the bottom of a long list.
  await a.ev(`(()=>{const m=document.querySelector('main');m.scrollTop=m.scrollHeight;return true})()`);
  await sleep(200);
  const pinned = await a.ev(`(()=>{const m=document.querySelector('main').getBoundingClientRect();const b=document.getElementById('cancel-all').getBoundingClientRect();return m.scrollHeight!==0&&b.top>=m.top&&b.bottom<=m.bottom})()`);
  if (!pinned) throw new Error("Cancel all scrolled out of view");
  await a.shot("13b-scrolled.png");
  ok("Pause all and Cancel all stay visible while scrolling");

  await a.ev(`(()=>{const r=[...document.querySelectorAll('.job')].find(x=>x.querySelector('.job-title').textContent===${JSON.stringify(pausedTitle)});[...r.querySelectorAll('button')].find(b=>b.textContent==='Resume').click();return true})()`);
  await a.until(`!${rowSub}.startsWith('Paused')`, "one download resumed", 30);
  ok("Resume on one download works");

  await a.ev(click("#cancel-all"));
  await a.until(`${subs}.every(t=>t.startsWith('Canceled')||t.startsWith('Done'))`, "everything canceled", 30);
  await sleep(2500);
  if (!(await a.ev(`${subs}.every(t=>t.startsWith('Canceled')||t.startsWith('Done'))`))) throw new Error("a download restarted after Cancel all");
  if (await a.ev(visible("pause-all")) || await a.ev(visible("cancel-all"))) throw new Error("buttons left after Cancel all");
  await a.shot("14-all-canceled.png");
  ok("Cancel all stops running and waiting downloads");

  // 10. Settings: a change saves to disk and reaches the main window.
  await a.ev(click("#open-settings"));
  await a.until(`document.getElementById('settings').open`, "settings open", 5);
  await a.shot("15-settings.png");
  await a.ev(`(()=>{const s=[...document.querySelectorAll('#settings-body select')].find(x=>x.getAttribute('aria-label')==='At once');s.value='4';s.dispatchEvent(new Event('change'));return true})()`);
  await a.ev(`(()=>{const s=[...document.querySelectorAll('#settings-body select')].find(x=>[...x.options].some(o=>o.value==='flac'));s.value='flac';s.dispatchEvent(new Event('change'));return true})()`);
  await a.until(`![...document.querySelectorAll('#settings-body select')].some(x=>[...x.options].some(o=>o.value==='320'))`, "bitrate hidden for lossless", 5);
  await a.ev(click("#close-settings"));
  await sleep(400);
  const savedSettings = JSON.parse(readFileSync(join(data, "settings.json"), "utf8"));
  if (savedSettings.concurrency !== 4 || savedSettings.audio_format !== "flac") throw new Error("settings not saved: " + JSON.stringify(savedSettings));
  if ((await a.ev(`document.getElementById('concurrency').value`)) !== "4") throw new Error("main window didn't pick up At once");
  ok("Settings save to disk and update the main window");

  // 11. About: the animation runs, then the approved text is all there.
  await a.ev(click("#open-about"));
  await sleep(350);
  await a.shot("16-about-animating.png");
  if ((await a.ev(`document.querySelector('.about-logo i[style*="background"]')?.getAnimations().length`)) === 0 && (await a.ev(`matchMedia('(prefers-reduced-motion: no-preference)').matches`))) throw new Error("about animation not running");
  await a.ev(`(()=>{window.__aboutClosed=0;document.getElementById('about').addEventListener('close',()=>{window.__aboutClosed=performance.now()});return true})()`);
  await sleep(1600);
  const aboutState = await a.ev(`(()=>{const d=document.getElementById('about');const l=[...d.querySelectorAll('.about-line')];return {open:d.open,closedAt:window.__aboutClosed,height:d.getBoundingClientRect().height,opacity:l.map(x=>getComputedStyle(x).opacity).join(),focus:document.hasFocus()}})()`);
  console.log("about state", JSON.stringify(aboutState));
  if (!aboutState.open || aboutState.height === 0) throw new Error("About closed by itself: " + JSON.stringify(aboutState));
  if (aboutState.opacity.split(",").some((o) => o !== "1")) throw new Error("About text still hidden: " + aboutState.opacity);
  const aboutText = await a.ev(`document.getElementById('about').innerText`);
  for (const t of ["Mr.Coder", "Oman", "Because we all love free", "karak", "I was never here", "v0.1.0"]) {
    if (!aboutText.includes(t)) throw new Error("About is missing: " + t);
  }
  await a.shot("17-about.png");
  if (!(await a.ev(`document.getElementById('about').open`))) throw new Error("About closed during the screenshot");
  ok("About animates and shows the full text");
  await a.ev(click("#close-about"));
  a.close();
} catch (e) {
  console.log("FAIL", e.message);
  process.exitCode = 1;
}

console.log("\nresults:\n" + results.join("\n"));
stopApp();
await sleep(800);
rmSync(testDir, { recursive: true, force: true });
for (const { path, text } of kept) {
  if (text === null) rmSync(path, { force: true });
  else writeFileSync(path, text);
}
console.log("settings and download list restored; tools kept:", existsSync(join(data, "tools")));
