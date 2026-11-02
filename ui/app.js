const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

let S = null;          // last snapshot
let openFp = null;     // expanded device row
let busy = {};         // fp -> pending mode change
let shownReq = null;   // pair request id in the modal
let dragging = null;

const MODES = [
  ["input", "Share Input", "This PC's keyboard & mouse", '<rect x="7" y="3" width="10" height="16" rx="5"/><path d="M12 3v5"/>'],
  ["extend", "Extend", "Extra screen", '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>'],
  ["mirror", "Duplicate", "Same screen", '<rect x="3" y="7" width="13" height="10" rx="1.5"/><path d="M8 4h11.5A1.5 1.5 0 0 1 21 5.5V13"/>'],
  ["off", "Off", "Not connected", '<circle cx="12" cy="12" r="8.5"/><path d="M8 12h8"/>'],
];
const LABEL = { input: "Share Input", extend: "Extend", mirror: "Duplicate", off: "Off" };
const ICON = {
  windows: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  macos: '<svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="11" rx="2"/><path d="M2 19h20"/></svg>',
  linux: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  tablet: '<svg viewBox="0 0 24 24"><rect x="4" y="3" width="16" height="18" rx="2.5"/><path d="M10.5 18h3"/></svg>',
  laptop: '<svg viewBox="0 0 24 24"><rect x="4.5" y="4.5" width="15" height="10.5" rx="1.5"/><path d="M2.5 18.5h19l-1.6-3.5H4.1z"/><path d="M10.5 17h3"/></svg>',
  desktop: '<svg viewBox="0 0 24 24"><rect x="2.5" y="3.5" width="19" height="13" rx="2"/><path d="M9 21h6M12 16.5V21"/></svg>',
};
const iconFor = (kind, os) => ICON[kind] || (os === "macos" ? ICON.laptop : ICON.desktop);
const osName = (o) => ({ windows: "Windows", macos: "macOS", linux: "Linux" }[o] || o || "");

function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.remove("hidden");
  clearTimeout(toast.t);
  toast.t = setTimeout(() => t.classList.add("hidden"), 5000);
}

async function call(cmd, args) {
  try { return await invoke(cmd, args); } catch (e) { toast(String(e)); throw e; }
}

// ---------- navigation ----------
function moveInd() {
  const b = document.querySelector(".nav button.on"), ind = $("#nav-ind");
  if (b && ind) { ind.style.top = b.offsetTop + "px"; ind.style.height = b.offsetHeight + "px"; }
}
requestAnimationFrame(moveInd);
document.querySelectorAll(".nav button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav button").forEach((x) => x.classList.toggle("on", x === b));
    moveInd();
    document.querySelectorAll(".view").forEach((v) => v.classList.toggle("on", v.id === b.dataset.view));
    render();
  })
);
document.querySelectorAll("[data-open]").forEach((b) => b.addEventListener("click", () => invoke("open_folder", { which: b.dataset.open })));

// ---------- devices ----------
// Views are patched in place (never rebuilt every tick) so clicks, focus and animations survive refreshes.
function setHTML(el, html) {
  if (el._h === html || el.contains(document.activeElement)) return;
  el._h = html;
  el.innerHTML = html;
}

// Something stuck? Disconnect and connect again with one click.
const reconnectBtn = (fp) => `<button class="btn" data-reconnect="${esc(fp)}">Reconnect</button>`;

// Sound plays on this computer only, unless the user also wants it on the device showing the screen.
const soundToggle = (key, name) => `<label class="snd"><input type="checkbox" data-sound="${esc(key)}" ${(S.sound || {})[key] ? "checked" : ""}/> Also play sound on ${esc(name)}</label>`;
// Pointer speed while the mouse is on this device (Windows only; bekdesk switches it as the mouse crosses over).
const speedSlider = (key, name) => {
  if (S.me.os !== "windows") return "";
  const v = (S.pointer || {})[key] || 1;
  return `<label class="spd">Mouse speed on ${esc(name)}<input type="range" min="0.5" max="3" step="0.25" value="${v}" data-speed="${esc(key)}" /><b>${v.toFixed(2).replace(/0$/, "")}×</b></label>`;
};

// Share Input (either way): hear that computer here, and/or play this computer's sound there.
// Ticking "play this PC's sound" on several computers plays it on all their speakers at once.
const canSend = (os) => os === "linux" || os === "windows";
const soundNote = (st, ok) => st ? `<div class="hint${st === "playing" ? " ok" : " warn"}">${st === "playing" ? ok : `No sound yet: ${esc(st)}`}</div>` : "";
const hearToggle = (d, controlling) => {
  if (!canSend(d.os)) return "";
  const on = (S.hear || []).includes(d.fp), st = on && (S.hearing || {})["pull:" + d.fp];
  const muted = controlling && d.os === "linux" ? ` (muted on ${esc(d.name)})` : "";
  return `<label class="snd"><input type="checkbox" data-hear="${esc(d.fp)}" ${on ? "checked" : ""}/> Play ${esc(d.name)}'s sound on this PC${muted}</label>${soundNote(st, `Playing ${esc(d.name)}'s sound here.`)}`;
};
// Microphone: the owner shares it first; then the other computer can use it as a microphone here.
const micToggles = (d) => {
  const share = (S.mic_share || []).includes(d.fp), use = (S.use_mic || []).includes(d.fp), st = use && (S.hearing || {})["mic:" + d.fp];
  const mine = canSend(S.me.os) ? `<label class="snd"><input type="checkbox" data-mic-share="${esc(d.fp)}" ${share ? "checked" : ""}/> Let ${esc(d.name)} use this PC's microphone</label>` : "";
  const theirs = canSend(d.os) ? `<label class="snd"><input type="checkbox" data-mic-use="${esc(d.fp)}" ${use ? "checked" : ""}/> Use ${esc(d.name)}'s microphone here${S.me.os === "linux" ? ` (pick "bekdesk-microphone" in your app)` : S.me.os === "windows" ? ` (pick "CABLE Output" in your app)` : ""}</label>${soundNote(st, `${esc(d.name)}'s microphone is on here.`)}` : "";
  return mine + theirs;
};
// Camera: the owner shares it first; then on Windows it appears as "bekdesk camera" in Zoom, Teams, browsers…
const camToggles = (d) => {
  const share = (S.cam_share || []).includes(d.fp), use = (S.use_cam || []).includes(d.fp), st = use && (S.camera || {})[d.fp];
  const mine = canSend(S.me.os) ? `<label class="snd"><input type="checkbox" data-cam-share="${esc(d.fp)}" ${share ? "checked" : ""}/> Let ${esc(d.name)} use this PC's camera</label>` : "";
  const theirs = canSend(d.os) && S.me.os === "windows" ? `<label class="snd"><input type="checkbox" data-cam-use="${esc(d.fp)}" ${use ? "checked" : ""}/> Use ${esc(d.name)}'s camera here (pick "bekdesk camera" in Zoom, Teams or your browser)</label>${soundNote(st, `${esc(d.name)}'s camera is on as "bekdesk camera".`)}` : "";
  return mine + theirs;
};
const pushToggle = (d) => {
  if (!canSend(S.me.os)) return "";
  const on = !!(S.sound || {})[d.fp], st = on && (S.hearing || {})["push:" + d.fp];
  return `<label class="snd"><input type="checkbox" data-sound="${esc(d.fp)}" ${on ? "checked" : ""}/> Also play this PC's sound on ${esc(d.name)}'s speakers</label>${soundNote(st, `This PC's sound plays on ${esc(d.name)} too.`)}`;
};
// Being controlled: stay awake so the screen doesn't blank and lock while you work from the other keyboard.
const awakeToggle = (d) => `<label class="snd"><input type="checkbox" data-awake="${esc(d.fp)}" ${(S.let_sleep || []).includes(d.fp) ? "" : "checked"}/> Keep this computer awake and unlocked while ${esc(d.name)} controls it</label>`;

const has = (engine) => !!(S.engines && S.engines[engine] && S.engines[engine].version);

// A computer's card body: the one thing to know right now and its one action, then the rest under "More".
function pcExtra(d, ses, mode, pill, sendFail) {
  const name = esc(d.name);
  const paired = d.trusted || d.once;
  const sharing = ses && ses.origin && ses.mode === "input", controlled = ses && !ses.origin && ses.mode === "input";
  let msg = "", act = "";
  if (!d.online) {
    msg = paired ? `<div class="hint">${name} is asleep, switched off, or bekdesk is closed on it. It reconnects by itself when it's back.</div>` : "";
    if (d.can_wake) act = `<button class="btn primary" data-wake="${esc(d.fp)}">Wake ${name}</button>`;
  }
  else if (d.locked) msg = `<div class="hint warn">${name} is locked. Unlock it on that computer to continue; while you share, bekdesk then keeps it awake so it doesn't lock again from being idle.</div>`;
  else if (!paired) msg = `<div class="hint">Pick a mode to connect. ${name} asks to approve; check both screens show <b>${esc(d.code)}</b>.</div>`;
  else if (sharing && S.me.scroll_lock) { msg = `<div class="hint warn">Scroll Lock is on, so the mouse stays on this PC.</div>`; act = `<button class="btn primary" data-scroll-lock="1">Turn off Scroll Lock</button>`; }
  else if (controlled) { msg = `<div class="hint ok">${name}'s keyboard and mouse are controlling this computer.</div>`; act = `<button class="btn primary" data-mode="input" data-fp="${esc(d.fp)}">Use this PC's keyboard &amp; mouse instead</button>`; }
  else if (sendFail) msg = sendFail;
  else if (sharing) msg = `<div class="hint">Move the mouse off the edge toward ${name}.</div>`;
  else if (!ses) msg = `<div class="hint">Drop files here to send them to ${name}.</div>`;
  // Something's wrong or still connecting: Reconnect is the obvious next step.
  const stuck = mode !== "off" && !busy[d.fp] && (d.note || pill[0] === "wait");
  if (stuck) act += act ? reconnectBtn(d.fp) : reconnectBtn(d.fp).replace('class="btn"', 'class="btn primary"');
  const more = [
    mode === "extend" || mode === "input" ? speedSlider(d.fp, d.name) : "",
    mode === "extend" || mode === "mirror" ? soundToggle(d.fp, d.name) : "",
    sharing || controlled ? hearToggle(d, sharing) : "",
    sharing || controlled ? pushToggle(d) : "",
    sharing || controlled ? micToggles(d) : "",
    sharing || controlled ? camToggles(d) : "",
    controlled ? awakeToggle(d) : "",
    mode !== "off" && !busy[d.fp] && !stuck ? `<div class="acts1">${reconnectBtn(d.fp)}</div>` : "",
    ses ? `<div class="hint">Drop files on this card to send them. Copied text and files sync while Share Input is on.</div>` : "",
    paired ? `<div class="acts1"><button class="link danger" data-forget-card="${esc(d.fp)}">Forget this computer</button></div>` : "",
  ].join("");
  return msg + (act ? `<div class="acts1">${act}</div>` : "") + (d.online && more ? `<details class="more"><summary>More</summary>${more}</details>` : "");
}

// One entry per thing you can connect: each nearby computer, plus the Android tablet.
function entities() {
  const out = [];
  const winHost = S.me.os === "windows";
  const devs = [...S.devices].sort((a, b) => (b.online - a.online) || String(a.name).localeCompare(String(b.name)));
  for (const d of devs) {
    const ses = d.session;
    const mode = busy[d.fp] || (ses && ses.origin ? ses.mode : d.mode) || "off";
    let pill;
    if (busy[d.fp]) pill = ["wait", busy[d.fp] === "off" ? "Disconnecting…" : "Connecting…"];
    else if (d.online && d.locked) pill = ["wait", "Locked"];
    else if (ses && !ses.origin && ses.mode === "input") pill = ["live", `Controlled by ${d.name}`];
    else if (ses && (ses.mode === "input" || ses.streaming || !ses.origin)) pill = ["live", `Connected · ${LABEL[ses.mode]}`];
    else if (ses) pill = ["wait", "Connecting…"];
    else if (!d.online) pill = ["off", d.trusted ? "Asleep or off" : "Offline"];
    else pill = ["ready", d.trusted || d.once ? "Ready" : "Not paired"];
    // What this computer can do (Linux can't share its own screen yet, but can show another computer's).
    const lastT = [...S.transfers].reverse().find((t) => t.peer === d.name);
    const sendFail = lastT && lastT.state === "failed" ? `<div class="hint warn">${lastT.outgoing ? "Sending" : "Receiving"} “${esc(lastT.label)}” failed. Check ${esc(d.name)} is still on and try again.</div>` : "";
    const host = has("sunshine") ? "" : "Sharing this computer's screen isn't available on Linux yet; pick Extend or Duplicate on the other computer to show its screen here";
    const no = { input: has("deskflow") ? "" : "Not available on this computer yet", extend: host, mirror: host };
    out.push({
      no,
      icon: iconFor(d.kind, d.os),
      key: d.fp, kind: "pc", online: d.online, name: d.name || "Unknown device", os: d.os, sub: osName(d.os) + (d.trusted ? " · paired" : d.once ? " · allowed until restart" : "") + (d.battery ? ` · battery ${d.battery.pct}%${d.battery.charging ? " (charging)" : d.battery.pct <= 15 ? " · low" : ""}` : ""), details: !!ses || !!d.note, pill, mode,
      modes: ["input", "extend", "mirror", "off"], disabled: !d.online || !!busy[d.fp], extendOk: winHost, note: d.note,
      extra: pcExtra(d, ses, mode, pill, sendFail),
    });
  }
  // Tablets and phones need the screen-sharing engine, which this computer may not have (Linux for now).
  document.querySelector(".addtab")?.classList.toggle("hidden", !has("sunshine"));
  if (!has("sunshine")) return out;
  const t = S.tablet || "off", ts = S.tablet_state || {};
  const host = `bekdesk ${S.me.name}`;
  // A Moonlight device that tapped this computer and is waiting for its PIN. Nothing is shared until a mode is picked.
  for (const p of ts.pending || []) {
    out.push({ key: "pair:" + p.id, kind: "tablet", online: true, name: "New tablet or phone", os: "tablet", sub: `A tablet or phone at ${p.address || "an unknown address"} wants to connect`, pill: ["wait", "Asking to pair"], mode: "off", modes: [], disabled: true, extendOk: false,
      extra: `<div class="hint">Type the 4-digit PIN shown on that device and give it a name. Nothing is shared until you choose Extend or Duplicate.</div>`
        + `<div class="pinrow" data-pair="${esc(p.id)}"><input class="pname" placeholder="Name, e.g. Galaxy Tab" maxlength="40" autocomplete="off" /><input class="ppin" inputmode="numeric" placeholder="PIN" maxlength="4" size="5" autocomplete="off" /><button class="btn primary" data-act="pair">Pair</button><button class="btn" data-act="decline">Decline</button></div>`
        + `<div class="hint">If the tablet says a pairing session already exists, press Decline and tap <b>${esc(host)}</b> again.</div>` });
  }
  const paired = ts.devices || [];
  if (t === "off" && !paired.length) return out;
  // Several paired devices share one stream: name the one showing now, list each on its own row.
  const now = ts.streaming ? paired.find((d) => d.name === ts.name) : null;
  const dn = now?.name || (paired.length === 1 ? paired[0].name : "") || "your tablet or phone";
  const mode = busy.tablet || t;
  let pill = ["ready", "Off"], extra = "";
  // Moonlight on Android streams 720p unless told otherwise, and only the device can change that.
  const sharp = `<div class="hint">For a sharp picture: in the app on the tablet open <b>Settings</b>, set <b>Video resolution</b> to <b>Native</b>${t === "extend" ? ` (${esc(S.tablet_res)})` : ""} move <b>Video bitrate</b> to about 40 Mbps, and set <b>Frame rate</b> to 90 on a 120 Hz screen (60 on a 60 Hz one).</div>`;
  const list = paired.map((d) => `<div class="mlrow"><span class="gi">${ICON.tablet}</span><b>${esc(d.name || "Unnamed device")}</b>${d === now ? `<span class="chip live">Showing now</span>` : `<span class="chip off">Not showing</span>`}<button class="link" data-act="forget" data-uuid="${esc(d.uuid)}">Remove</button></div>`).join("");
  const devRows = paired.length > 1 ? `<div class="mlist">${list}</div><div class="hint">One device shows this PC's screen at a time.</div>` : "";
  const more = (items) => `<details class="more"><summary>More</summary>${items.join("")}</details>`;
  const fix401 = paired.length ? `<div class="hint">The tablet says <b>not authorized (401)</b>? On the tablet long-press <b>${esc(host)}</b> → <b>Delete PC</b>, then tap it again and pair with the PIN.</div>` : "";
  if (t !== "off") {
    if (!ts.running) pill = ["wait", "Starting…"];
    else if (ts.streaming) pill = ["live", `Connected · ${LABEL[t]}`];
    else pill = ["wait", "Not connected"];
    extra = ts.streaming ? `<div class="hint ok">Showing on ${esc(dn)} now.</div>`
      : paired.length ? `<div class="hint">On ${esc(dn)}, open Moonlight, tap <b>${esc(host)}</b>, then <b>Desktop</b>.</div>`
      : `<div class="hint">On the tablet or phone, open Moonlight and tap <b>${esc(host)}</b>. It asks for a PIN here.</div>`;
    if (S.tablet_note && !busy.tablet) extra += `<div class="acts1">${reconnectBtn("tablet").replace('class="btn"', 'class="btn primary"')}</div>`;
    extra = devRows + extra;
    extra += more([
      paired.length > 1 ? "" : list, sharp, fix401,
      t === "extend" ? speedSlider("tablet", dn) : "", soundToggle("tablet", dn),
      t === "extend" ? `<label class="resrow">Screen size <input id="tab-res" value="${esc(S.tablet_res)}" size="10" /></label>` : "",
      !busy.tablet && !S.tablet_note ? `<div class="acts1">${reconnectBtn("tablet")}</div>` : "",
    ]);
  } else {
    extra = devRows + `<div class="hint">Choose Extend or Duplicate, then open Moonlight on it.</div>` + more([paired.length > 1 ? "" : list, fix401]);
  }
  out.push({ key: "tablet", kind: "tablet", online: true, name: paired.length > 1 ? "Tablets and phones" : dn, os: "tablet", sub: paired.length > 1 ? `${paired.length} paired` + (t === "off" ? "" : t === "extend" ? " · extra screen for this PC" : " · same screen as this PC") : t === "off" ? "Tablet or phone" : t === "extend" ? "Extra screen for this PC" : "Same screen as this PC", pill, mode,
    modes: winHost ? ["extend", "mirror", "off"] : ["mirror", "off"], disabled: !!busy.tablet, extendOk: winHost, note: S.tablet_note, extra });
  return out;
}

const cards = new Map();
function buildCard(e) {
  const el = document.createElement("div");
  el.className = "dev rise";
  el.dataset.fp = e.key;
  el.innerHTML = `<div class="head"><div class="ico"></div><div class="t"><div class="n"></div><div class="s"></div></div><span class="pill"><i></i><b></b></span></div>
    <div class="seg">${e.modes.map((m) => { const x = MODES.find((q) => q[0] === m); return `<button data-mode="${m}" data-fp="${esc(e.key)}"><svg class="mi" viewBox="0 0 24 24">${x[3]}</svg><b>${x[1]}</b><span>${x[2]}</span></button>`; }).join("")}</div>
    <div class="note"></div><div class="extra"></div>${e.key.startsWith("pair:") ? "" : `<details class="logs"><summary>Details</summary><pre>Loading…</pre></details>`}`;
  return el;
}
function renderDevices() {
  const list = $("#device-list");
  const ents = entities();
  const comp = S.components;
  const cb = $("#components");
  cb.classList.toggle("hidden", !comp);
  if (comp) {
    const pct = /(\d+)%/.exec(comp);
    cb.querySelector("span").textContent = comp.includes("Couldn't") ? comp : `Getting bekdesk ready — ${comp}`;
    cb.querySelector("i").style.width = (pct ? pct[1] : 8) + "%";
    cb.classList.toggle("err", comp.includes("Couldn't"));
  }
  const keep = new Set();
  ents.forEach((e, i) => {
    keep.add(e.key);
    let el = cards.get(e.key);
    if (!el) { el = buildCard(e); cards.set(e.key, el); list.appendChild(el); }
    el.classList.toggle("tab", e.kind === "tablet");
    setHTML(el.querySelector(".ico"), e.icon || ICON.tablet);
    el.querySelector(".n").textContent = e.name;
    el.querySelector(".s").textContent = e.sub;
    const pill = el.querySelector(".pill");
    pill.className = "pill " + e.pill[0];
    pill.querySelector("b").textContent = e.pill[1];
    const live = e.pill[0] === "live";
    if (live && el._live === false) { el.classList.remove("joined"); void el.offsetWidth; el.classList.add("joined"); }
    el._live = live;
    el.classList.toggle("live", live);
    el.querySelectorAll(".seg [data-mode]").forEach((b) => {
      const m = b.dataset.mode, noExt = m === "extend" && !e.extendOk;
      b.classList.toggle("on", e.mode === m);
      const why = (e.no && e.no[m]) || (noExt ? "Extend from macOS isn't available yet" : "");
      b.disabled = !e.online || (e.disabled && e.mode !== m) || !!why;
      b.title = why;
    });
    const note = el.querySelector(".note");
    note.textContent = e.note || "";
    note.classList.toggle("on", !!e.note);
    setHTML(el.querySelector(".extra"), e.extra);
    const det = el.querySelector("details.logs");
    if (det) det.hidden = e.kind === "pc" && !e.details;
    if (det && det.open && Date.now() - (det._at || 0) > 2000) loadDetails(det, e.key);
    if (list.children[i] !== el) list.insertBefore(el, list.children[i] || null);
  });
  for (const [k, el] of cards) if (!keep.has(k)) { el.remove(); cards.delete(k); }
  $("#empty").classList.toggle("hidden", S.devices.length > 0);
  $("#empty .small").innerHTML = `This computer is <b>${esc(S.me.name)}</b>. Searching…`;
}

$("#device-list").addEventListener("click", async (e) => {
  const m = e.target.closest("[data-mode]");
  if (m && !m.disabled) {
    const fp = m.dataset.fp, mode = m.dataset.mode;
    busy[fp] = mode;
    render();
    try {
      if (fp === "tablet") await call("set_tablet", { mode, res: $("#tab-res") ? $("#tab-res").value : S.tablet_res });
      else await call("set_mode", { fp, mode });
    } finally { delete busy[fp]; refresh(); }
    return;
  }
  const act = e.target.closest("[data-act]");
  if (act) pairAction(act);
  const r = e.target.closest("[data-reconnect]");
  if (r) reconnect(r.dataset.reconnect);
  const wk = e.target.closest("[data-wake]");
  if (wk) { try { await invoke("wake", { fp: wk.dataset.wake }); toast("Wake signal sent. It can take a minute to start."); } catch (err) { toast(`Couldn't wake it: ${err}`); } return; }
  if (e.target.closest("[data-scroll-lock]")) { await call("toggle_scroll_lock"); setTimeout(refresh, 300); return; }
  const f = e.target.closest("[data-forget-card]");
  if (f && armed(f, "Press again to forget it")) { await call("forget", { fp: f.dataset.forgetCard }); toast("Forgotten. It asks for the code again next time."); refresh(); }
});
// Card → Details: the last log lines about this connection, refreshed while it's open.
async function loadDetails(det, key) {
  det._at = Date.now();
  try { det.querySelector("pre").textContent = await invoke("device_details", { key }); } catch (_) {}
}
$("#device-list").addEventListener("toggle", (e) => {
  const det = e.target.closest && e.target.closest("details.logs");
  if (det && det.open) loadDetails(det, det.closest(".dev").dataset.fp);
}, true);
async function reconnect(fp) {
  const mode = fp === "tablet" ? S.tablet : (S.devices.find((d) => d.fp === fp) || {}).mode;
  if (!mode || mode === "off") return;
  busy[fp] = mode;
  render();
  try {
    if (fp === "tablet") { await call("set_tablet", { mode: "off", res: S.tablet_res }); await new Promise((ok) => setTimeout(ok, 2500)); await call("set_tablet", { mode, res: S.tablet_res }); }
    else { await call("set_mode", { fp, mode: "off" }); await call("set_mode", { fp, mode }); }
    toast("Reconnected");
  } finally { delete busy[fp]; refresh(); }
}
let lastScan = 0;
async function scan(manual) {
  if (!manual && Date.now() - lastScan < 15000) return;
  lastScan = Date.now();
  const b = $("#scan");
  b.disabled = true; b.textContent = "Scanning…";
  try { await invoke("rescan"); } catch (_) {}
  b.disabled = false; b.textContent = "Scan";
  refresh();
}
$("#scan").addEventListener("click", () => scan(true));
// Check for devices when bekdesk is opened or brought back, not constantly in the background.
document.addEventListener("visibilitychange", () => { if (!document.hidden) scan(false); });
window.addEventListener("focus", () => scan(false));
// Two-step confirm inside the button: browser confirm() pop-ups don't reliably appear in the app window.
function armed(b, text) {
  if (b._armed) return true;
  b._armed = true;
  const was = b.textContent;
  b.textContent = text;
  b.classList.add("armed");
  setTimeout(() => { b._armed = false; b.textContent = was; b.classList.remove("armed"); }, 4000);
  return false;
}
async function pairAction(b) {
  const row = b.closest("[data-pair]");
  try {
    if (b.dataset.act === "forget") {
      if (!armed(b, "Press again to remove")) return;
      await call("tablet_forget", { uuid: b.dataset.uuid });
      toast("Removed");
    } else if (b.dataset.act === "decline") {
      await call("tablet_decline", { id: row.dataset.pair });
    } else if (b.dataset.act === "pair") {
      const name = row.querySelector(".pname"), pin = row.querySelector(".ppin");
      if (!name.value.trim()) { name.focus(); toast("Give the device a name"); return; }
      if (pin.value.length !== 4) { pin.focus(); return; }
      b.disabled = true;
      await call("tablet_pin", { id: row.dataset.pair, pin: pin.value, name: name.value });
      toast(`${name.value.trim()} paired`);
    }
  } catch (_) { b.disabled = false; }
  refresh();
}
$("#device-list").addEventListener("input", (e) => {
  if (e.target.dataset.speed) { e.target.nextElementSibling.textContent = (+e.target.value).toFixed(2).replace(/0$/, "") + "×"; return; }
  if (e.target.classList.contains("ppin")) e.target.value = e.target.value.replace(/\D/g, "");
});
$("#device-list").addEventListener("change", async (e) => {
  if (e.target.dataset.speed) {
    const key = e.target.dataset.speed, factor = +e.target.value;
    S.pointer = { ...(S.pointer || {}), [key]: factor };
    call("set_pointer", { key, factor });
    return;
  }
  if (e.target.dataset.hear) {
    const fp = e.target.dataset.hear, on = e.target.checked;
    S.hear = on ? [...(S.hear || []), fp] : (S.hear || []).filter((x) => x !== fp);
    call("set_hear", { fp, on });
    return;
  }
  if (e.target.dataset.awake) {
    const fp = e.target.dataset.awake, on = e.target.checked;
    S.let_sleep = on ? (S.let_sleep || []).filter((x) => x !== fp) : [...(S.let_sleep || []), fp];
    call("set_stay_awake", { fp, on });
    return;
  }
  if (e.target.dataset.camShare || e.target.dataset.camUse) {
    const share = !!e.target.dataset.camShare, fp = e.target.dataset.camShare || e.target.dataset.camUse, on = e.target.checked;
    const k = share ? "cam_share" : "use_cam";
    S[k] = on ? [...(S[k] || []), fp] : (S[k] || []).filter((x) => x !== fp);
    call("set_cam", { fp, share, on });
    return;
  }
  if (e.target.dataset.micShare || e.target.dataset.micUse) {
    const share = !!e.target.dataset.micShare, fp = e.target.dataset.micShare || e.target.dataset.micUse, on = e.target.checked;
    const k = share ? "mic_share" : "use_mic";
    S[k] = on ? [...(S[k] || []), fp] : (S[k] || []).filter((x) => x !== fp);
    call("set_mic", { fp, share, on });
    return;
  }
  if (e.target.dataset.sound) {
    const key = e.target.dataset.sound, on = e.target.checked;
    await call("set_sound", { key, on });
    S.sound = { ...(S.sound || {}), [key]: on };
    // A screen stream picks up the sound setting when it starts; Share Input sound starts by itself.
    const d = S.devices.find((x) => x.fp === key);
    if (key === "tablet" || (d && d.session && d.session.mode !== "input")) reconnect(key);
    return;
  }
  if (e.target.id === "tab-res") call("set_tablet", { mode: S.tablet, res: e.target.value }).then(refresh);
});

// Files dropped onto a device card are sent to that device.
// Only drops on this window: the edge drop zones are separate windows with their own handling.
const thisWin = window.__TAURI__.webviewWindow?.getCurrentWebviewWindow?.();
const onWin = (ev, f) => (thisWin ? thisWin.listen(ev, f) : listen(ev, f));
onWin("tauri://drag-over", (e) => {
  const p = e.payload.position, r = window.devicePixelRatio || 1;
  const card = document.elementFromPoint(p.x / r, p.y / r)?.closest(".dev[data-fp]");
  document.querySelectorAll(".dev.drop").forEach((c) => c !== card && c.classList.remove("drop"));
  card?.classList.add("drop");
});
onWin("tauri://drag-leave", () => document.querySelectorAll(".dev.drop").forEach((c) => c.classList.remove("drop")));
onWin("tauri://drag-drop", async (e) => {
  const p = e.payload.position, r = window.devicePixelRatio || 1;
  document.querySelectorAll(".dev.drop").forEach((c) => c.classList.remove("drop"));
  const card = document.elementFromPoint(p.x / r, p.y / r)?.closest(".dev[data-fp]");
  if (!card || !e.payload.paths?.length) return;
  const fp = card.dataset.fp;
  toast(`Sending ${e.payload.paths.length} item(s)…`);
  await call("send_files", { fp, paths: e.payload.paths });
  toast("Sent");
});

// ---------- pairing requests ----------
function renderModal() {
  const r = S.requests[0];
  const m = $("#modal");
  if (!r) { m.classList.add("hidden"); shownReq = null; return; }
  if (shownReq === r.id) return;
  shownReq = r.id;
  m.innerHTML = `<div class="dialog" role="dialog" aria-modal="true">
    <h3>${esc(r.name)} wants to connect</h3>
    <div class="small">${esc(osName(r.os))} · ${esc(r.fp.slice(0, 16))}…<br/>Only continue if the other screen shows the same code.</div>
    <div class="code">${esc(r.code)}</div>
    <div class="acts">
      <button class="btn" data-d="once" title="Asks again next time bekdesk starts">Only this time</button>
      <button class="btn primary" data-d="always">Always Allow</button>
      <button class="btn" data-d="decline">Decline</button>
      <button class="btn danger" data-d="block">Block</button>
    </div></div>`;
  m.classList.remove("hidden");
  window.focus();
}
$("#modal").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-d]");
  if (!b) return;
  await call("decide", { id: S.requests[0].id, decision: b.dataset.d });
  $("#modal").classList.add("hidden");
  refresh();
});

// ---------- manage screens ----------
// Positions are stored in workspace units (1 unit = 10 physical px). This computer is drawn at the
// centre of the canvas; every other device keeps its offset from it.
function monitorsOf(s) {
  return String(s || "").split(";").map((m) => m.split(",").map(Number)).filter((v) => v.length === 4 && v[2] > 0);
}
function boxOf(mons) {
  const x0 = Math.min(...mons.map((m) => m[0])), y0 = Math.min(...mons.map((m) => m[1]));
  const x1 = Math.max(...mons.map((m) => m[0] + m[2])), y1 = Math.max(...mons.map((m) => m[1] + m[3]));
  return { x0, y0, w: (x1 - x0) / 10, h: (y1 - y0) / 10 };
}
function resMon(res, fallback) {
  const m = /^(\d+)\s*[xX]\s*(\d+)$/.exec(String(res || "").trim());
  return m ? [[0, 0, +m[1], +m[2]]] : fallback;
}
const KIND_LABEL = { laptop: "Laptop", desktop: "Desktop", tablet: "Tablet" };

function wsItems() {
  const items = [{ key: "self", name: S.me.name, os: S.me.os, kind: S.me.kind || "desktop", mons: monitorsOf(S.me.monitors), me: true, online: true, state: "This PC" }];
  for (const d of S.devices) {
    if (!d.trusted && !d.once && !d.online) continue;
    const st = d.session ? LABEL[d.session.mode] : d.online ? (d.trusted || d.once ? "Ready" : "Not paired") : "Offline";
    items.push({ key: d.fp, name: d.name || "Unknown", os: d.os, kind: d.kind || (d.os === "macos" ? "laptop" : "desktop"), mons: monitorsOf(d.monitors), online: d.online, live: !!d.session, state: st });
  }
  // Shown as live only while it is actually streaming; otherwise greyed out as "Not connected".
  const tsw = S.tablet_state || {};
  if (S.tablet && S.tablet !== "off") items.push({ key: "tablet", name: tsw.name || "Tablet or phone", os: "android", kind: "tablet", mons: resMon(S.tablet_res, [[0, 0, 2560, 1600]]), online: !!tsw.streaming, live: !!tsw.streaming, state: tsw.streaming ? LABEL[S.tablet] : "Not connected",
 });
  for (const it of items) {
    if (!it.mons.length) { it.mons = [[0, 0, it.kind === "laptop" ? 1920 : 2560, it.kind === "laptop" ? 1200 : 1440]]; it.guess = true; }
    it.b = boxOf(it.mons);
  }
  // Devices without a saved spot go around this PC: right, left, below, above, then corners.
  const L = S.layout || {};
  const me = items[0];
  me.p = L.self || { x: 0, y: 0 };
  const spots = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [-1, 1], [1, -1], [-1, -1]];
  let k = 0;
  for (const it of items.slice(1)) {
    if (L[it.key]) { it.p = L[it.key]; continue; }
    const [sx, sy] = spots[k++ % spots.length];
    it.p = {
      // gaps leave room for monitor stands and name labels
      x: Math.round(me.p.x + (sx > 0 ? me.b.w + 24 : sx < 0 ? -it.b.w - 24 : (me.b.w - it.b.w) / 2)),
      y: Math.round(me.p.y + (sy > 0 ? me.b.h + 70 : sy < 0 ? -it.b.h - 70 : (me.b.h - it.b.h) / 2)),
    };
  }
  return items;
}

// Windows' details (connection, monitor name, refresh, rotation) for this PC's screen whose corner is at (x, y).
const screenAt = (x, y) => (S.me.screens || []).find((d) => d.x === x && d.y === y);

// Screens are sized in percent of their group, so resizing the window only animates the group box.
function frameHTML(it) {
  const PW = it.b.w * 10, PH = it.b.h * 10;
  const screens = it.mons.map((m, i) => {
    const l = ((m[0] - it.b.x0) / PW) * 100, t = ((m[1] - it.b.y0) / PH) * 100, w = (m[2] / PW) * 100, h = (m[3] / PH) * 100;
    const info = it.me ? screenAt(m[0], m[1]) : null;
    const conn = info && info.conn && info.conn !== "Other" ? `<em class="conn">${esc(info.conn)}</em>` : "";
    // Only this computer's screens are numbered (matching Identify and Windows); others show name and size.
    const tag = it.me ? `<span class="num">${i + 1}${i === 0 ? '<em>Main</em>' : ""}${conn}</span>` : "";
    const stand = it.kind === "desktop" ? `<span class="neck"></span><span class="foot"></span>` : "";
    const pos = it.me ? ` data-x="${m[0]}" data-y="${m[1]}" data-w="${m[2]}" data-h="${m[3]}" data-i="${i}" title="${esc(info && info.label || "")}"` : "";
    return `<div class="screen ${it.kind}"${pos} style="left:${l}%;top:${t}%;width:${w}%;height:${h}%"><i></i>${tag}<span class="res">${m[2]}×${m[3]}${it.guess ? "?" : ""}</span>${stand}</div>`;
  }).join("");
  const base = it.kind === "laptop" ? `<span class="kbd"></span>` : "";
  const ico = iconFor(it.kind, it.os);
  const chip = it.me ? "me" : it.live ? "live" : it.online ? "" : "off";
  return `${screens}${base}<div class="glabel"><div class="gn"><span class="gi">${ico}</span><b>${esc(it.name)}</b></div><div class="gc"><span class="chip ${chip}">${esc(it.state)}</span><span class="kind">${KIND_LABEL[it.kind] || ""}</span></div></div>`;
}

const groups = new Map();
let wsView = null;
function renderWorkspace() {
  if (dragging) return;
  const c = $("#canvas");
  const W = c.clientWidth, H = c.clientHeight;
  if (!W) return;
  const items = wsItems();
  const me = items[0];
  let cx = me.p.x + me.b.w / 2, cy = me.p.y + me.b.h / 2;
  // Fit everything while keeping this PC centred; leave room for stands and labels.
  let ex = me.b.w / 2, ey = me.b.h / 2;
  for (const it of items) {
    ex = Math.max(ex, Math.abs(it.p.x - cx), Math.abs(it.p.x + it.b.w - cx));
    ey = Math.max(ey, Math.abs(it.p.y - cy), Math.abs(it.p.y + it.b.h - cy));
  }
  // Keep the view still while arranging, so dropping one device never shrinks or shifts the others;
  // refit only when the canvas size or the set of devices changes, or something would fall off the edge.
  const fit = Math.min(1.1, (W / 2 - 40) / ex, (H / 2 - 70) / ey), keys = items.map((i) => i.key).join();
  if (!wsView || wsView.W !== W || wsView.H !== H || wsView.keys !== keys || wsView.scale > fit * 1.02) wsView = { W, H, keys, scale: fit, cx, cy };
  const { scale } = wsView;
  ({ cx, cy } = wsView);
  Object.assign(c.dataset, { scale, cx, cy });
  const keep = new Set();
  for (const it of items) {
    keep.add(it.key);
    let g = groups.get(it.key);
    if (!g) {
      g = document.createElement("div");
      g.dataset.key = it.key;
      c.appendChild(g);
      groups.set(it.key, g);
    }
    g.className = `group k-${it.kind} ${it.me ? "me" : ""} ${it.online === false ? "offline" : ""} ${g._seen ? "" : "pop"}`;
    g._seen = true;
    g._l = W / 2 + (it.p.x - cx) * scale; g._t = H / 2 - 20 + (it.p.y - cy) * scale; g._w = it.b.w * scale; g._ht = it.b.h * scale;
    Object.assign(g.style, { left: g._l + "px", top: g._t + "px", width: g._w + "px", height: g._ht + "px" });
    g._b = it.b;
    setHTML(g, frameHTML(it));
  }
  for (const [k, g] of groups) if (!keep.has(k)) { g.remove(); groups.delete(k); }
  $("#ws-hint").classList.toggle("hidden", items.length > 1);
}

$("#identify").addEventListener("click", () => call("identify_screens"));
// Right-click one of this computer's screens: make it the main display, identify, or open display settings.
function hideCtx() { $("#ctx").classList.add("hidden"); }
// Resolution list for one screen, inside the same menu.
async function showModes(at, info) {
  const m = $("#ctx");
  m.innerHTML = `<div class="ctx-h"><b>Resolution &amp; refresh rate</b><span>Now ${info.w}×${info.h} · ${info.hz} Hz</span></div><div class="ctx-list">Loading…</div>`;
  try {
    const { modes } = await invoke("display_modes", at);
    const sideways = info.orientation % 2 === 1;
    m.querySelector(".ctx-list").innerHTML = modes.map(([w, h, hz]) => {
      const now = (sideways ? h === info.w && w === info.h : w === info.w && h === info.h) && hz === info.hz;
      return `<button data-act="mode" data-w="${w}" data-h="${h}" data-hz="${hz}" class="${now ? "on" : ""}">${w} × ${h} <span>${hz} Hz</span></button>`;
    }).join("") || "No other modes";
  } catch (e) { m.querySelector(".ctx-list").textContent = String(e); }
}
// Display changes can leave a monitor black; like Windows, undo them unless the user confirms within 15 seconds.
async function changeAndConfirm(at, change, undo) {
  try { await call("change_display", { ...at, ...change }); } catch (_) { return; }
  refresh();
  const k = $("#keep");
  let left = 15;
  const tick = () => { k.querySelector("b").textContent = `Reverting in ${left} s`; };
  tick();
  k.classList.remove("hidden");
  const done = (keep) => {
    clearInterval(t);
    k.classList.add("hidden");
    if (!keep) call("change_display", { ...at, ...undo }).then(refresh, () => {});
  };
  const t = setInterval(() => { if (--left <= 0) done(false); else tick(); }, 1000);
  k.querySelector("[data-k=keep]").onclick = () => done(true);
  k.querySelector("[data-k=revert]").onclick = () => done(false);
}
$("#canvas").addEventListener("contextmenu", (e) => {
  e.preventDefault();
  const sc = e.target.closest(".group.me .screen");
  const m = $("#ctx");
  const main = sc && sc.dataset.i === "0";
  const info = sc && screenAt(+sc.dataset.x, +sc.dataset.y);
  const head = info ? `<div class="ctx-h"><b>Screen ${+sc.dataset.i + 1}${info.label ? " · " + esc(info.label) : ""}</b><span>${esc(info.conn)} · ${info.w}×${info.h} · ${info.hz} Hz</span></div>` : "";
  m.innerHTML = head + (sc ? `<button data-act="primary" ${main ? "disabled" : ""}>${main ? "This is the main display" : `Make screen ${+sc.dataset.i + 1} the main display`}</button>` : "")
    + (info ? `<button data-act="modes">Resolution &amp; refresh rate…</button>
      <div class="ctx-row">Rotate ${["Landscape", "Portrait", "Landscape (flipped)", "Portrait (flipped)"].map((t, o) => `<button data-act="rotate" data-o="${o}" class="${info.orientation === o ? "on" : ""}" title="${t}">${["▭", "▯", "▭↻", "▯↻"][o]}</button>`).join("")}</div>
      ${main ? "" : `<button data-act="off">Turn off this screen</button>`}` : "")
    + `<button data-act="identify">Identify screens</button><button data-act="settings">Display settings…</button>`;
  m._sc = sc;
  m.style.left = Math.min(e.clientX, innerWidth - 260) + "px";
  m.style.top = Math.min(e.clientY, innerHeight - 300) + "px";
  m.classList.remove("hidden");
});
$("#ctx").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-act]");
  if (!b || b.disabled) return;
  const sc = $("#ctx")._sc, at = sc && { x: +sc.dataset.x, y: +sc.dataset.y }, info = sc && screenAt(at.x, at.y);
  if (b.dataset.act === "modes") return showModes(at, info);
  hideCtx();
  if (b.dataset.act === "primary") { await call("make_primary", at); toast("Main display changed"); }
  if (b.dataset.act === "mode") return changeAndConfirm(at, { w: +b.dataset.w, h: +b.dataset.h, hz: +b.dataset.hz }, { w: info.w, h: info.h, hz: info.hz });
  if (b.dataset.act === "rotate") return changeAndConfirm(at, { orientation: +b.dataset.o }, { orientation: info.orientation });
  if (b.dataset.act === "off") {
    if (!armed(b, "Press again to turn off")) { $("#ctx").classList.remove("hidden"); return; }
    await call("change_display", { ...at, off: true }); toast("Screen turned off. Win+P → Extend turns it back on."); refresh();
  }
  if (b.dataset.act === "identify") call("identify_screens");
  if (b.dataset.act === "settings") invoke("open_display_settings");
});
// A click that rebuilt the menu (e.g. opening the resolution list) leaves its target detached: not an outside click.
document.addEventListener("click", (e) => { if (e.target.isConnected && !e.target.closest("#ctx")) hideCtx(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") hideCtx(); });
$("#canvas").addEventListener("pointerdown", (e) => {
  const g = e.target.closest(".group");
  if (!g || e.button !== 0) return;
  // One of this PC's other screens (e.g. an HDMI monitor): move just that screen, as in Windows display settings.
  const sc = e.target.closest(".group.me .screen");
  if (sc && sc.dataset.i !== "0") {
    sc.setPointerCapture(e.pointerId);
    dragging = { mon: sc, g, sx: e.clientX, sy: e.clientY, l: sc.offsetLeft, t: sc.offsetTop, moved: false };
    sc.classList.add("dragging");
    return;
  }
  g.setPointerCapture(e.pointerId);
  dragging = { g, sx: e.clientX, sy: e.clientY, l: g._l, t: g._t, moved: false };
  g.classList.add("dragging");
  g.style.left = g._l + "px"; g.style.top = g._t + "px";
});
$("#canvas").addEventListener("pointermove", (e) => {
  if (!dragging) return;
  dragging.moved = true;
  if (dragging.mon) {
    dragging.mon.style.left = dragging.l + e.clientX - dragging.sx + "px";
    dragging.mon.style.top = dragging.t + e.clientY - dragging.sy + "px";
    return;
  }
  dragging.g.style.left = dragging.l + e.clientX - dragging.sx + "px";
  dragging.g.style.top = dragging.t + e.clientY - dragging.sy + "px";
});
/** v moved to the nearest of `edges` if that is within `near`, else v unchanged. */
function snapTo(v, edges, near) {
  const e = edges.reduce((b, c) => (Math.abs(c - v) < Math.abs(b - v) ? c : b), Infinity);
  return Math.abs(e - v) <= near ? e : v;
}
console.assert(snapTo(100, [105, 300], 8) === 105 && snapTo(100, [120], 8) === 100 && snapTo(5, [], 8) === 5, "snapTo");
async function endMonitorDrag({ mon, g, moved }) {
  dragging = null;
  mon.classList.remove("dragging");
  if (!moved) return;
  // Snap flush against the nearest edge of another of this PC's screens, aligning edges when close.
  let l = mon.offsetLeft, t = mon.offsetTop, best = null;
  for (const o of g.querySelectorAll(".screen")) {
    if (o === mon) continue;
    const top = Math.abs(t - o.offsetTop) < 5 ? o.offsetTop : t, left = Math.abs(l - o.offsetLeft) < 5 ? o.offsetLeft : l;
    for (const [x, y] of [[o.offsetLeft + o.offsetWidth, top], [o.offsetLeft - mon.offsetWidth, top], [left, o.offsetTop + o.offsetHeight], [left, o.offsetTop - mon.offsetHeight]]) {
      const d = Math.hypot(x - l, y - t);
      if (!best || d < best.d) best = { x, y, d };
    }
  }
  if (best) { l = best.x; t = best.y; }
  const k = (g._b.w * 10) / g.offsetWidth; // desktop pixels per canvas pixel
  let nx = Math.round(g._b.x0 + l * k), ny = Math.round(g._b.y0 + t * k);
  // Canvas rounding leaves a few pixels off; land exactly on the other screens' edges.
  const [w, h] = [+mon.dataset.w, +mon.dataset.h];
  const snap = (v, size, edges) => edges.flatMap((e) => [e, e - size]).reduce((b, c) => (Math.abs(c - v) < Math.abs(b - v) ? c : b), Infinity);
  const others = [...g.querySelectorAll(".screen")].filter((o) => o !== mon).map((o) => o.dataset);
  const sx = snap(nx, w, others.flatMap((o) => [+o.x, +o.x + +o.w])), sy = snap(ny, h, others.flatMap((o) => [+o.y, +o.y + +o.h]));
  if (Math.abs(sx - nx) < 12) nx = sx;
  if (Math.abs(sy - ny) < 12) ny = sy;
  try {
    await call("move_display", { x: +mon.dataset.x, y: +mon.dataset.y, nx, ny });
  } catch (_) {}
  g._h = null; // redraw this PC from the real arrangement, whether or not Windows accepted it
  refresh();
}
async function endDrag() {
  if (!dragging) return;
  if (dragging.mon) return endMonitorDrag(dragging);
  const { g, moved } = dragging;
  g.classList.remove("dragging");
  if (!moved) { dragging = null; return; }
  const c = $("#canvas"), scale = +c.dataset.scale, cx = +c.dataset.cx, cy = +c.dataset.cy, W = c.clientWidth, H = c.clientHeight;
  let gl = parseFloat(g.style.left), gt = parseFloat(g.style.top);
  // The device stays where it was dropped; each axis only clicks onto a neighbour's edge when already within a few pixels.
  const others = [...groups.values()].filter((o) => o !== g);
  gl = snapTo(gl, others.flatMap((o) => [o._l + o._w, o._l - g._w, o._l, o._l + o._w - g._w]), 8);
  gt = snapTo(gt, others.flatMap((o) => [o._t, o._t + o._ht - g._ht, o._t + o._ht, o._t - g._ht]), 8);
  // Everything else keeps its current spot; only the dragged device moves.
  const layout = {};
  for (const it of wsItems()) layout[it.key] = { x: Math.round(it.p.x), y: Math.round(it.p.y) };
  layout[g.dataset.key] = { x: Math.round((gl - W / 2) / scale + cx), y: Math.round((gt - (H / 2 - 20)) / scale + cy) };
  dragging = null;
  S.layout = layout;
  renderWorkspace();
  await call("save_layout", { layout });
  refresh();
}
$("#canvas").addEventListener("pointerup", endDrag);
$("#canvas").addEventListener("pointercancel", endDrag);

// ---------- activity ----------
const fmtBytes = (n) => n > 1e9 ? (n / 1e9).toFixed(2) + " GB" : n > 1e6 ? (n / 1e6).toFixed(1) + " MB" : n > 1e3 ? (n / 1e3).toFixed(0) + " KB" : n + " B";
const fmtTime = (s) => s > 3600 ? `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}m` : s > 60 ? `${Math.floor(s / 60)}m` : `${s}s`;

// ---------- shared files ----------
// Everything other computers sent here, newest first; open it, find it, or pass it on to another computer.
let galleryAt = 0, galleryKey = "";
async function renderGallery() {
  const key = S.transfers.map((t) => t.id + t.state).join();
  if (key === galleryKey && Date.now() - galleryAt < 15000) return;
  galleryKey = key; galleryAt = Date.now();
  let items = [];
  try { items = await invoke("shared_files"); } catch (_) {}
  const icon = `<svg viewBox="0 0 24 24"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/></svg>`;
  setHTML($("#gallery"), items.map((f, i) => `<div class="gitem" data-i="${i}">
      <div class="gthumb">${f.thumb ? `<img src="${f.thumb}" alt="" loading="lazy" />` : icon}</div>
      <div class="gname" title="${esc(f.name)}">${esc(f.name)}</div>
      <div class="small">from ${esc(f.from)} · ${fmtBytes(f.size)}</div>
      <div class="gacts"><button class="link" data-g="open">Open</button><button class="link" data-g="reveal">Show</button><button class="link" data-g="send">Send on</button></div>
    </div>`).join("") || `<div class="empty">Files other computers send you show up here. They're saved in Downloads/bekdesk.</div>`);
  $("#gallery")._items = items;
}
$("#gallery").addEventListener("click", (e) => {
  const b = e.target.closest("[data-g]"), it = e.target.closest(".gitem");
  if (!b || !it) return;
  const f = $("#gallery")._items[+it.dataset.i];
  if (b.dataset.g === "send") return pickTargets([f.path]);
  call("open_shared", { path: f.path, reveal: b.dataset.g === "reveal" });
});

// Choose which computers get these files (right-click → Send with bekdesk, or "Send on" above).
function pickTargets(paths) {
  const devs = S.devices.filter((d) => d.online && (d.trusted || d.once));
  const m = $("#sendto");
  const names = paths.map((p) => p.split(/[\\/]/).pop());
  m.innerHTML = `<div class="dialog" role="dialog" aria-modal="true">
    <h3>Send ${paths.length === 1 ? esc(names[0]) : `${paths.length} files`}</h3>
    <div class="small">${devs.length ? "Pick the computers to send to." : "No paired computer is online right now. Open bekdesk on the other computer and try again."}</div>
    <div class="pick">${devs.map((d) => `<label class="snd"><input type="checkbox" value="${esc(d.fp)}" checked /> ${esc(d.name)}</label>`).join("")}</div>
    <div class="acts"><button class="btn primary" data-s="go" ${devs.length ? "" : "disabled"}>Send</button><button class="btn" data-s="cancel">Cancel</button></div></div>`;
  m.classList.remove("hidden");
  m.onclick = async (e) => {
    const b = e.target.closest("[data-s]");
    if (!b) return;
    const fps = [...m.querySelectorAll("input:checked")].map((i) => i.value);
    m.classList.add("hidden");
    if (b.dataset.s !== "go") return;
    for (const fp of fps) call("send_files", { fp, paths }).catch((err) => toast(`Couldn't send: ${err}`));
    toast(`Sending to ${fps.length} computer${fps.length === 1 ? "" : "s"}…`);
  };
}
async function checkPendingSend() {
  try { const p = await invoke("take_pending_send"); if (p.length) pickTargets(p); } catch (_) {}
}
listen("send-pending", checkPendingSend);

function renderActivity() {
  renderGallery();
  const live = S.devices.filter((d) => d.session);
  const ts = S.tablet_state || {};
  const tab = S.tablet && S.tablet !== "off"
    ? `<div class="card row2"><span class="dot ${ts.streaming ? "live" : ""}"></span><div class="grow"><b>${esc(ts.name || "Tablet or phone")}</b> · ${LABEL[S.tablet]} (from this computer)<div class="small">${ts.streaming ? "Streaming now" : ts.paired ? `Not connected · open the app on it and tap bekdesk ${esc(S.me.name)}` : "Nothing paired yet"}</div></div></div>` : "";
  setHTML($("#sessions"), live.map((d) => `<div class="card row2"><span class="dot live"></span><div class="grow"><b>${esc(d.name)}</b> · ${LABEL[d.session.mode]} ${d.session.origin ? "(from this computer)" : "(from " + esc(d.name) + ")"}<div class="small">${esc(d.session.status)} · ${esc(d.session.ip)} · ${fmtTime(d.session.uptime)}</div></div></div>`).join("") + tab
    || `<div class="empty">Nothing is connected right now. Pick Share Input, Extend or Duplicate on a device in Nearby Devices.</div>`);
  setHTML($("#transfers"), [...S.transfers].reverse().slice(0, 30).map((t) => `<div class="card"><div class="row2"><div class="grow"><b>${esc(t.label)}</b> <span class="small">${t.outgoing ? "to" : "from"} ${esc(t.peer)} · ${esc(t.state)}</span></div><span class="small">${fmtBytes(t.done)} / ${fmtBytes(t.total)}</span></div><div class="bar"><i style="width:${t.total ? (100 * t.done) / t.total : 100}%"></i></div></div>`).join("")
    || `<div class="empty">Nothing sent yet. Drop files on a device in Nearby Devices, or with Share Input on, copy files on one computer and paste them on the other. Received files go to Downloads/bekdesk.</div>`);
  $("#log").textContent = S.log.map((l) => { const [ts, ...r] = l.split(" "); return new Date(+ts * 1000).toLocaleTimeString() + "  " + r.join(" "); }).join("\n");
}

$("#diag").addEventListener("click", async () => {
  const text = await call("diagnostics");
  try { await navigator.clipboard.writeText(text); toast("Diagnostics copied — paste them in your message"); }
  catch (_) { toast("Couldn't copy; use Open logs instead"); }
});

// ---------- settings ----------
const PERM = {
  input: ["Control this computer", "Allow this device to control your keyboard and mouse."],
  files: ["Send files here", "Allow this device to send files to this computer."],
  clipboard: ["Clipboard", "Allow clipboard sync between devices."],
  screen: ["Show its screen here", "Allow this device to share its screen or be an extra display."],
};
function renderSettings() {
  if (document.activeElement !== $("#name")) $("#name").value = S.me.name;
  const sc = S.scroll || { speed: 1, invert: false };
  if (document.activeElement !== $("#scroll-speed")) $("#scroll-speed").value = sc.speed;
  $("#scroll-val").textContent = (+$("#scroll-speed").value).toFixed(2).replace(/0$/, "") + "×";
  $("#scroll-invert").checked = sc.invert;
  // Windows only: the lock screen helper.
  $("#lock-card").classList.toggle("hidden", S.lock_screen == null);
  if (S.lock_screen != null && !lockBusy) {
    $("#lock-text").textContent = S.lock_screen
      ? "On. While another computer controls this one, its keyboard and mouse also work on the lock screen and on Windows admin prompts, and other computers' cameras can show up here as \"bekdesk camera\"."
      : "Off. Turn this on so a computer controlling this one can type your password on the lock screen, and so other computers' cameras can show up here as \"bekdesk camera\" (Windows asks for admin permission once).";
    $("#lock-btn").textContent = S.lock_screen ? "Turn off" : "Turn on";
    $("#lock-btn").className = S.lock_screen ? "btn" : "btn primary";
  }
  const devs = S.devices.filter((d) => d.trusted || d.online);
  setHTML($("#trusted"), devs.map((d) => `<div class="srow" data-fp="${esc(d.fp)}"><div class="row2"><div class="ico sm dark">${iconFor(d.kind, d.os)}</div><div class="grow"><b>${esc(d.name)}</b><div class="small">${esc(osName(d.os))} · ${d.online ? "Online" : "Offline"} · ${d.trusted ? "Trusted" : d.once ? "Allowed this session" : "Not paired"}</div></div>
      ${d.trusted ? "" : d.online ? `<button class="btn primary" data-pair="${esc(d.fp)}">Ask to connect</button>` : ""}
      ${d.trusted ? `<button class="btn" data-forget="${esc(d.fp)}">Forget</button>` : ""}<button class="btn danger" data-block="${esc(d.fp)}">Block</button></div>
    ${d.trusted ? `<details class="perm" open><summary>Permissions</summary><div class="small">Choose what this device can do when it's connected to you.</div><div class="caps">${["input", "files", "clipboard", "screen"].map((k) => `<label><input type="checkbox" data-cap="${k}" ${d.caps[k] ? "checked" : ""}/><span><b>${PERM[k][0]}</b><small>${PERM[k][1]}</small></span></label>`).join("")}</div></details>` : ""}</div>`).join("")
    || `<div class="empty">No devices found yet. Open bekdesk on your other computer, on the same Wi-Fi.</div>`);
  setHTML($("#links"), Object.entries(LINKS).filter(([, u]) => u).map(([n, u]) => `<button class="btn" data-url="${esc(u)}">${esc(n)}</button>`).join(""));
  setHTML($("#blocked"), Object.entries(S.blocked).map(([fp, n]) => `<div class="card row2"><div class="grow"><b>${esc(n)}</b> <span class="small">${esc(fp.slice(0, 16))}…</span></div><button class="btn" data-unblock="${esc(fp)}">Unblock</button></div>`).join("")
    || `<div class="small">Nothing blocked.</div>`);
}
$("#name").addEventListener("change", (e) => call("set_name", { name: e.target.value }));
// ---------- first run ----------
// Shown once for this version: what each system asks for, before the first connection needs it.
const WELCOME = "welcome-0.1.22";
const STEPS = {
  windows: ["When Windows asks about the firewall, allow bekdesk on <b>Private networks</b>.", "Keep bekdesk open (it can sit in the tray) on every computer you want to use."],
  linux: ["Open the firewall once: <code>sudo firewall-cmd --permanent --add-port=47290/tcp --add-service=mdns && sudo firewall-cmd --reload</code>", "When another computer first takes control, your desktop asks to <b>allow remote control</b>. Choose <b>Allow</b> and tick <b>Remember</b> if offered."],
  macos: ["When asked, allow bekdesk in <b>System Settings → Privacy & Security → Accessibility</b> (to share the keyboard and mouse) and <b>Screen Recording</b> (to show screens)."],
};
function showWelcome() {
  let seen = true;
  try { seen = !!localStorage.getItem(WELCOME); } catch (_) {}
  if (seen || !S) return false;
  $("#wl-name").value = S.me.name;
  setHTML($("#wl-steps"), (STEPS[S.me.os] || []).map((t) => `<li>${t}</li>`).join(""));
  $("#wl-admin-row").classList.toggle("hidden", S.lock_screen == null || S.lock_screen);
  $("#welcome").classList.remove("hidden");
  return true;
}
$("#wl-go").addEventListener("click", async () => {
  const name = $("#wl-name").value.trim();
  if (name && name !== S.me.name) await call("set_name", { name });
  await call("autostart", { enable: $("#wl-auto").checked });
  try { localStorage.setItem(WELCOME, "1"); } catch (_) {}
  $("#welcome").classList.add("hidden");
  // Runs after the parts finish downloading; the Windows prompt appears then.
  if (!$("#wl-admin-row").classList.contains("hidden") && $("#wl-admin").checked) adminSetup(true);
  refresh();
});

// While parts are downloading nothing works yet, so the window waits behind a friendly screen.
const WAIT_MSGS = ["Be patient, something good is downloading…", "Fetching the keyboard-and-mouse helper…", "Packing the screen-sharing parts…", "Almost there. Time for a cup of chiya?", "Setting up so your first connection is instant."];
let grSkip = false, grTick = 0;
function renderGetReady() {
  const comp = S.components || "", busy = !!comp && !grSkip && $("#welcome").classList.contains("hidden");
  $("#getready").classList.toggle("hidden", !busy);
  if (!busy) return;
  const failed = comp.includes("Couldn't");
  const pct = /(\d+)%/.exec(comp);
  $("#getready .bar i").style.width = (pct ? pct[1] : 6) + "%";
  $("#getready .gr-detail").textContent = comp;
  $("#gr-skip").classList.toggle("hidden", !failed);
  $("#getready .gr-msg").textContent = failed ? "No internet? bekdesk keeps trying every minute." : WAIT_MSGS[Math.floor(grTick++ / 4) % WAIT_MSGS.length];
}
$("#gr-skip").addEventListener("click", () => { grSkip = true; renderGetReady(); });

// Appearance: the sidebar button flips light/dark; Settings also offers "Auto" (follow the system).
function renderTheme() {
  const p = window.themePref?.() || "auto";
  document.querySelectorAll("#theme-seg [data-theme]").forEach((b) => b.classList.toggle("on", b.dataset.theme === p));
  $("#theme-btn span").textContent = document.documentElement.classList.contains("dark") ? "Light mode" : "Dark mode";
}
$("#theme-seg").addEventListener("click", (e) => { const b = e.target.closest("[data-theme]"); if (b) { setTheme(b.dataset.theme); renderTheme(); } });
$("#lock-all").addEventListener("click", () => call("lock_all"));
$("#theme-btn").addEventListener("click", () => { setTheme(document.documentElement.classList.contains("dark") ? "light" : "dark"); renderTheme(); });
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", renderTheme);
renderTheme();
let lockBusy = false;
async function adminSetup(enable) {
  lockBusy = true;
  $("#lock-btn").disabled = true;
  $("#lock-btn").textContent = enable ? "Setting up… approve the Windows prompt" : "Removing…";
  try {
    const on = await invoke("admin_setup", { enable });
    toast(on ? "Lock screen support is on" : "Lock screen support is off");
  } catch (e) { toast(`Couldn't change lock screen support: ${e}`); }
  lockBusy = false;
  $("#lock-btn").disabled = false;
  refresh();
}
$("#lock-btn").addEventListener("click", () => adminSetup(!S.lock_screen));
const saveScroll = () => call("set_scroll", { speed: +$("#scroll-speed").value, invert: $("#scroll-invert").checked }).then(refresh);
$("#scroll-speed").addEventListener("input", () => ($("#scroll-val").textContent = (+$("#scroll-speed").value).toFixed(2).replace(/0$/, "") + "×"));
$("#scroll-speed").addEventListener("change", saveScroll);
$("#scroll-invert").addEventListener("change", saveScroll);
$("#autostart").addEventListener("change", (e) => call("autostart", { enable: e.target.checked }));
// Only links that are filled in are shown.
const LINKS = { GitHub: "https://github.com/pabibek9/bekdesk", LinkedIn: "", Instagram: "", "Join the developer team": "" };
$("#links").addEventListener("click", (e) => { const b = e.target.closest("[data-url]"); if (b) invoke("open_url", { url: b.dataset.url }); });
$("#trusted").addEventListener("click", async (e) => {
  const pb = e.target.closest("[data-pair]"), bb = e.target.closest("[data-block]");
  if (pb) { toast("Waiting for the other computer to accept…"); await call("pair_device", { fp: pb.dataset.pair }); toast("Connected"); refresh(); return; }
  if (bb && armed(bb, "Press again to block")) { await call("block_device", { fp: bb.dataset.block }); refresh(); return; }
  if (e.target.dataset.forget) { await call("forget", { fp: e.target.dataset.forget }); refresh(); }
});
$("#trusted").addEventListener("change", (e) => {
  const card = e.target.closest("[data-fp]");
  if (!e.target.dataset.cap || !card) return;
  const caps = {};
  card.querySelectorAll("[data-cap]").forEach((c) => (caps[c.dataset.cap] = c.checked));
  call("set_caps", { fp: card.dataset.fp, caps });
});
$("#blocked").addEventListener("click", async (e) => {
  if (e.target.dataset.unblock) { await call("unblock", { fp: e.target.dataset.unblock }); refresh(); }
});

// ---------- loop ----------
let renaming = false;
function renderMe() {
  const m = S.me, el = $("#me");
  if (renaming) return;
  setHTML(el, `<button class="mename" id="rename" title="Rename this computer"><b>${esc(m.name)}</b><svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4"/></svg></button>
    <div class="meid">${osName(m.os)} · ID ${esc(m.id || "")}</div>`);
  $("#addr-ip").textContent = m.ip || "Not connected to a network";
  $("#addr-id").textContent = `${m.name} · ID ${m.id || ""}`;
  $("#copy-addr").disabled = !m.ip;
}
$("#me").addEventListener("click", (e) => {
  if (!e.target.closest("#rename") || renaming) return;
  renaming = true;
  const el = $("#me");
  el._h = null;
  el.innerHTML = `<input id="rename-in" maxlength="60" value="${esc(S.me.name)}" aria-label="Computer name" />
    <div class="meid">Enter to save · Esc to cancel</div>`;
  const i = $("#rename-in");
  i.focus(); i.select();
  const done = async (save) => {
    if (!renaming) return;
    renaming = false;
    const v = i.value.trim();
    i.blur();
    if (save && v && v !== S.me.name) { await call("set_name", { name: v }); toast(`Renamed to ${v}`); }
    refresh();
  };
  i.addEventListener("keydown", (k) => { if (k.key === "Enter") done(true); if (k.key === "Escape") done(false); });
  i.addEventListener("blur", () => done(true));
});
// Sidebar: the version in use, and an update button when a newer one is out.
function renderVersion() {
  const u = S.update || {};
  const status = u.available
    ? `<button class="btn primary sm" id="do-update">Update to ${esc(u.available)}</button>`
    : u.error
      ? `<span class="ver-err" title="${esc(u.error)}">Update check failed</span><button class="link" id="check-update">Retry</button>`
      : `<button class="link" id="check-update">Check for updates</button>`;
  setHTML($("#ver"), `<span>bekdesk ${esc(S.version || "")}</span><span class="ver-s">${status}</span>`);
}
$("#ver").addEventListener("click", async (e) => {
  if (e.target.id === "check-update") {
    e.target.textContent = "Checking…";
    try { const v = await invoke("check_update"); toast(v ? `bekdesk ${v} is available` : "You have the newest version"); } catch (err) { toast(`Couldn't check for updates: ${err}`); }
    refresh();
  }
  if (e.target.id === "do-update") {
    e.target.disabled = true; e.target.textContent = "Downloading…";
    try { await call("install_update"); } catch (_) { e.target.disabled = false; refresh(); }
  }
});
$("#quit").addEventListener("click", (e) => { if (armed(e.currentTarget, "Press again to quit")) invoke("quit_app"); });
$("#copy-addr").addEventListener("click", async () => {
  try { await navigator.clipboard.writeText(S.me.ip); toast(`Copied ${S.me.ip}`); } catch (_) { toast(`Your address is ${S.me.ip}`); }
});
$("#connect-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const i = $("#connect-host"), b = $("#connect-btn");
  if (!i.value.trim()) return;
  b.disabled = true; b.textContent = "Connecting…";
  try { const name = await call("connect_address", { host: i.value }); toast(`Found ${name}`); i.value = ""; openFp = null; }
  catch (_) {}
  b.disabled = false; b.textContent = "Connect";
  refresh();
});

function render() {
  if (!S) return;
  renderMe();
  renderVersion();
  const v = document.querySelector(".view.on").id;
  if (v === "devices") renderDevices();
  if (v === "workspace") renderWorkspace();
  if (v === "activity") renderActivity();
  if (v === "settings") renderSettings();
  renderModal();
  if (!welcomed) welcomed = true, showWelcome();
  renderGetReady();
}
let welcomed = false;
setTimeout(checkPendingSend, 1500); // started with right-click → Send with bekdesk
async function refresh() {
  try { S = await invoke("snapshot"); render(); } catch (e) { console.error(e); }
}
invoke("autostart", {}).then((on) => ($("#autostart").checked = on)).catch(() => {});
refresh();
setInterval(refresh, 1000);
window.addEventListener("resize", () => { render(); moveInd(); });
