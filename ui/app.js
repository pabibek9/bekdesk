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
  ["input", "Share Input", "One mouse and keyboard"],
  ["extend", "Extend", "Extra monitor"],
  ["mirror", "Duplicate", "Same screen on both"],
  ["off", "Off", "Disconnect"],
];
const LABEL = { input: "Share Input", extend: "Extend", mirror: "Duplicate", off: "Off" };
const ICON = {
  windows: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  macos: '<svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="11" rx="2"/><path d="M2 19h20"/></svg>',
  linux: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  tablet: '<svg viewBox="0 0 24 24"><rect x="5" y="3" width="14" height="18" rx="2"/><path d="M11 18h2"/></svg>',
};
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
document.querySelectorAll(".nav button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav button").forEach((x) => x.classList.toggle("on", x === b));
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

function status(d) {
  if (d.session) return d.session.origin ? `${LABEL[d.session.mode]} · ${d.session.status}` : d.session.status;
  if (busy[d.fp]) return busy[d.fp] === "off" ? "Disconnecting…" : `Connecting · ${LABEL[busy[d.fp]]}…`;
  if (!d.online) return d.trusted ? "Offline · trusted" : "Offline";
  if (d.trusted) return "Ready · trusted";
  if (d.once) return "Ready · allowed this session";
  return "Not paired yet";
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
    else if (ses && (ses.mode === "input" || ses.streaming || !ses.origin)) pill = ["live", `Connected · ${LABEL[ses.mode]}`];
    else if (ses) pill = ["wait", "Connecting…"];
    else if (!d.online) pill = ["off", "Offline"];
    else pill = ["ready", d.trusted || d.once ? "Ready" : "Not paired"];
    out.push({
      key: d.fp, kind: "pc", online: d.online, name: d.name || "Unknown device", os: d.os, sub: `${osName(d.os)} · ${status(d)}`, pill, mode,
      modes: ["input", "extend", "mirror", "off"], disabled: !d.online || !!busy[d.fp], extendOk: winHost, note: d.note,
      extra: !d.trusted && !d.once && d.online
        ? `<div class="hint">First connection asks for approval on ${esc(d.name)}. Check that both screens show code <b>${esc(d.code)}</b>.</div>`
        : d.online ? `<div class="hint">Drop files on this card to send them to ${esc(d.name)}. Copied files and text sync while Share Input is on.</div>` : "",
    });
  }
  const t = S.tablet || "off", ts = S.tablet_state || {};
  const mode = busy.tablet || t;
  let pill = ["ready", "Off"], extra = "";
  const steps = `<ol class="steps"><li>Install <b>Moonlight</b> on the tablet (Play Store or F-Droid).</li><li>Open it and tap <b>${esc(S.me.name)}</b>.</li><li>Type the 4-digit PIN it shows here:</li></ol>`;
  if (t !== "off") {
    if (!ts.running) pill = ["wait", "Starting…"];
    else if (ts.streaming) pill = ["live", `Connected · ${LABEL[t]}`];
    else if (ts.paired) pill = ["wait", "Paired · waiting for Moonlight"];
    else pill = ["wait", "Waiting for the tablet"];
    const pin = `<div class="pinrow"><input id="tab-pin" inputmode="numeric" placeholder="PIN" maxlength="4" size="5" autocomplete="off" /><button class="btn" id="tab-pair">Pair</button></div>`;
    extra = ts.streaming ? `<div class="hint ok">Streaming to the tablet now.</div>`
      : ts.paired ? `<div class="hint">Paired. In Moonlight tap <b>${esc(S.me.name)}</b>, then <b>Desktop</b>.</div>`
      : steps + pin;
    if (t === "extend") extra += `<label class="resrow">Tablet screen size <input id="tab-res" value="${esc(S.tablet_res)}" size="10" /></label>`;
  } else {
    extra = `<div class="hint">There is no bekdesk app for Android yet: the tablet uses the free Moonlight app. Choose Extend or Duplicate to begin.</div>`;
  }
  out.push({ key: "tablet", kind: "tablet", online: true, name: "Android tablet", os: "tablet", sub: S.tablet_status || (t === "off" ? "Uses the Moonlight app" : t === "extend" ? "Extra monitor for this PC" : "Showing this PC's screen"), pill, mode,
    modes: winHost ? ["extend", "mirror", "off"] : ["mirror", "off"], disabled: !!busy.tablet, extendOk: winHost, note: S.tablet_note, extra });
  return out;
}

const cards = new Map();
function buildCard(e) {
  const el = document.createElement("div");
  el.className = "dev rise";
  el.dataset.fp = e.key;
  el.innerHTML = `<div class="head"><div class="ico"></div><div class="t"><div class="n"></div><div class="s"></div></div><span class="pill"><i></i><b></b></span></div>
    <div class="seg">${e.modes.map((m) => { const x = MODES.find((q) => q[0] === m); return `<button data-mode="${m}" data-fp="${esc(e.key)}"><b>${x[1]}</b><span>${x[2]}</span></button>`; }).join("")}</div>
    <div class="note"></div><div class="extra"></div>`;
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
    setHTML(el.querySelector(".ico"), ICON[e.os] || ICON.windows);
    el.querySelector(".n").textContent = e.name;
    el.querySelector(".s").textContent = e.sub;
    const pill = el.querySelector(".pill");
    pill.className = "pill " + e.pill[0];
    pill.querySelector("b").textContent = e.pill[1];
    el.classList.toggle("live", e.pill[0] === "live");
    el.querySelectorAll("[data-mode]").forEach((b) => {
      const m = b.dataset.mode, noExt = m === "extend" && !e.extendOk;
      b.classList.toggle("on", e.mode === m);
      b.disabled = !e.online || (e.disabled && e.mode !== m) || noExt;
      b.title = noExt ? "Extend from macOS isn't available yet" : "";
    });
    const note = el.querySelector(".note");
    note.textContent = e.note || "";
    note.classList.toggle("on", !!e.note);
    setHTML(el.querySelector(".extra"), e.extra);
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
  if (e.target.id === "tab-pair") submitPin();
});
async function submitPin() {
  const i = $("#tab-pin");
  if (!i || i.value.length !== 4) return;
  const pin = i.value;
  i.blur();
  try { await call("tablet_pin", { pin }); toast("Tablet paired"); i.value = ""; } catch (_) { i.focus(); }
  refresh();
}
$("#device-list").addEventListener("input", (e) => {
  if (e.target.id === "tab-pin") { e.target.value = e.target.value.replace(/\D/g, ""); if (e.target.value.length === 4) submitPin(); }
});
$("#device-list").addEventListener("change", (e) => {
  if (e.target.id === "tab-res") call("set_tablet", { mode: S.tablet, res: e.target.value }).then(refresh);
});

// Files dropped onto a device card are sent to that device.
listen("tauri://drag-over", (e) => {
  const p = e.payload.position, r = window.devicePixelRatio || 1;
  const card = document.elementFromPoint(p.x / r, p.y / r)?.closest(".dev[data-fp]");
  document.querySelectorAll(".dev.drop").forEach((c) => c !== card && c.classList.remove("drop"));
  card?.classList.add("drop");
});
listen("tauri://drag-leave", () => document.querySelectorAll(".dev.drop").forEach((c) => c.classList.remove("drop")));
listen("tauri://drag-drop", async (e) => {
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
      <button class="btn" data-d="once">Allow Once</button>
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
  if (S.tablet && S.tablet !== "off") items.push({ key: "tablet", name: "Android tablet", os: "android", kind: "tablet", mons: resMon(S.tablet_res, [[0, 0, 2560, 1600]]), online: true, live: true, state: S.tablet === "extend" ? "Extend" : "Mirror" });
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

// Screens are sized in percent of their group, so resizing the window only animates the group box.
function frameHTML(it) {
  const PW = it.b.w * 10, PH = it.b.h * 10;
  const screens = it.mons.map((m, i) => {
    const l = ((m[0] - it.b.x0) / PW) * 100, t = ((m[1] - it.b.y0) / PH) * 100, w = (m[2] / PW) * 100, h = (m[3] / PH) * 100;
    const tag = it.kind === "tablet" ? "" : `<span class="num">${i + 1}${i === 0 ? '<em>Main</em>' : ""}</span>`;
    const stand = it.kind === "desktop" ? `<span class="neck"></span><span class="foot"></span>` : "";
    return `<div class="screen ${it.kind}" style="left:${l}%;top:${t}%;width:${w}%;height:${h}%"><i></i>${tag}<span class="res">${m[2]}×${m[3]}${it.guess ? "?" : ""}</span>${stand}</div>`;
  }).join("");
  const base = it.kind === "laptop" ? `<span class="kbd"></span>` : "";
  const ico = ICON[it.kind === "tablet" ? "tablet" : it.os] || ICON.windows;
  const chip = it.me ? "me" : it.live ? "live" : it.online ? "" : "off";
  return `${screens}${base}<div class="glabel"><div class="gn"><span class="gi">${ico}</span><b>${esc(it.name)}</b></div><div class="gc"><span class="chip ${chip}">${esc(it.state)}</span><span class="kind">${KIND_LABEL[it.kind] || ""}</span></div></div>`;
}

const groups = new Map();
function renderWorkspace() {
  if (dragging) return;
  const c = $("#canvas");
  const W = c.clientWidth, H = c.clientHeight;
  if (!W) return;
  const items = wsItems();
  const me = items[0];
  const cx = me.p.x + me.b.w / 2, cy = me.p.y + me.b.h / 2;
  // Fit everything while keeping this PC centred; leave room for stands and labels.
  let ex = me.b.w / 2, ey = me.b.h / 2;
  for (const it of items) {
    ex = Math.max(ex, Math.abs(it.p.x - cx), Math.abs(it.p.x + it.b.w - cx));
    ey = Math.max(ey, Math.abs(it.p.y - cy), Math.abs(it.p.y + it.b.h - cy));
  }
  const scale = Math.min(1.1, (W / 2 - 40) / ex, (H / 2 - 70) / ey);
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
    g._l = W / 2 + (it.p.x - cx) * scale; g._t = H / 2 - 20 + (it.p.y - cy) * scale; g._w = it.b.w * scale; g._h = it.b.h * scale;
    Object.assign(g.style, { left: g._l + "px", top: g._t + "px", width: g._w + "px", height: g._h + "px" });
    setHTML(g, frameHTML(it));
  }
  for (const [k, g] of groups) if (!keep.has(k)) { g.remove(); groups.delete(k); }
  $("#ws-hint").classList.toggle("hidden", items.length > 1);
}

$("#identify").addEventListener("click", () => call("identify_screens"));
$("#canvas").addEventListener("pointerdown", (e) => {
  const g = e.target.closest(".group");
  if (!g) return;
  g.setPointerCapture(e.pointerId);
  dragging = { g, sx: e.clientX, sy: e.clientY, l: g._l, t: g._t, moved: false };
  g.classList.add("dragging");
  g.style.left = g._l + "px"; g.style.top = g._t + "px";
});
$("#canvas").addEventListener("pointermove", (e) => {
  if (!dragging) return;
  dragging.moved = true;
  dragging.g.style.left = dragging.l + e.clientX - dragging.sx + "px";
  dragging.g.style.top = dragging.t + e.clientY - dragging.sy + "px";
});
async function endDrag() {
  if (!dragging) return;
  const { g, moved } = dragging;
  g.classList.remove("dragging");
  if (!moved) { dragging = null; return; }
  const c = $("#canvas"), scale = +c.dataset.scale, cx = +c.dataset.cx, cy = +c.dataset.cy, W = c.clientWidth, H = c.clientHeight;
  let gl = parseFloat(g.style.left), gt = parseFloat(g.style.top);
  // Snap flush to the nearest edge of another device, aligning tops/lefts when close.
  let best = null;
  for (const o of groups.values()) {
    if (o === g) continue;
    const top = Math.abs(gt - o._t) < 30 ? o._t : gt, left = Math.abs(gl - o._l) < 30 ? o._l : gl;
    for (const [x, y] of [[o._l + o._w, top], [o._l - g._w, top], [left, o._t + o._h], [left, o._t - g._h]]) {
      const dist = Math.hypot(x - gl, y - gt);
      if (!best || dist < best.dist) best = { x, y, dist };
    }
  }
  if (best && best.dist < 40) { gl = best.x; gt = best.y; }
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

function renderActivity() {
  const live = S.devices.filter((d) => d.session);
  setHTML($("#sessions"), live.map((d) => `<div class="card row2"><span class="dot live"></span><div class="grow"><b>${esc(d.name)}</b> · ${LABEL[d.session.mode]} ${d.session.origin ? "(from this computer)" : "(from " + esc(d.name) + ")"}<div class="small">${esc(d.session.status)} · ${esc(d.session.ip)} · ${fmtTime(d.session.uptime)}</div></div></div>`).join("")
    || `<div class="empty">No active connections.</div>`);
  setHTML($("#transfers"), [...S.transfers].reverse().slice(0, 30).map((t) => `<div class="card"><div class="row2"><div class="grow"><b>${esc(t.label)}</b> <span class="small">${t.outgoing ? "to" : "from"} ${esc(t.peer)} · ${esc(t.state)}</span></div><span class="small">${fmtBytes(t.done)} / ${fmtBytes(t.total)}</span></div><div class="bar"><i style="width:${t.total ? (100 * t.done) / t.total : 100}%"></i></div></div>`).join("")
    || `<div class="empty">No transfers yet.</div>`);
  $("#log").textContent = S.log.map((l) => { const [ts, ...r] = l.split(" "); return new Date(+ts * 1000).toLocaleTimeString() + "  " + r.join(" "); }).join("\n");
}

// ---------- settings ----------
function renderSettings() {
  if (document.activeElement !== $("#name")) $("#name").value = S.me.name;
  const trusted = S.devices.filter((d) => d.trusted);
  setHTML($("#trusted"), trusted.map((d) => `<div class="card" data-fp="${esc(d.fp)}"><div class="row2"><div class="grow"><b>${esc(d.name)}</b> <span class="small">${esc(osName(d.os))} · ${esc(d.fp.slice(0, 16))}…</span></div><button class="btn danger" data-forget="${esc(d.fp)}">Forget</button></div>
    <div class="caps">${["input", "clipboard", "files", "screen"].map((k) => `<label><input type="checkbox" data-cap="${k}" ${d.caps[k] ? "checked" : ""}/> ${{ input: "Control this computer", clipboard: "Clipboard", files: "Send files here", screen: "Show its screen here" }[k]}</label>`).join("")}</div></div>`).join("")
    || `<div class="empty">No trusted devices yet.</div>`);
  setHTML($("#blocked"), Object.entries(S.blocked).map(([fp, n]) => `<div class="card row2"><div class="grow"><b>${esc(n)}</b> <span class="small">${esc(fp.slice(0, 16))}…</span></div><button class="btn" data-unblock="${esc(fp)}">Unblock</button></div>`).join("")
    || `<div class="empty">Nothing blocked.</div>`);
  setHTML($("#engines"), Object.entries(S.engines).map(([k, v]) => `<div class="card row2"><div class="grow"><b>${{ deskflow: "Deskflow (input)", sunshine: "Sunshine (display host)", moonlight: "Moonlight (display viewer)", vdd: "Virtual Display Driver" }[k]}</b> ${v.version ? esc(v.version) : "— not for this OS"}</div><span>${v.installed ? "Installed" : v.version ? "Downloading automatically" : ""}</span></div>`).join(""));
}
$("#name").addEventListener("change", (e) => call("set_name", { name: e.target.value }));
$("#autostart").addEventListener("change", (e) => call("autostart", { enable: e.target.checked }));
$("#trusted").addEventListener("click", async (e) => {
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
  const v = document.querySelector(".view.on").id;
  if (v === "devices") renderDevices();
  if (v === "workspace") renderWorkspace();
  if (v === "activity") renderActivity();
  if (v === "settings") renderSettings();
  renderModal();
}
async function refresh() {
  try { S = await invoke("snapshot"); render(); } catch (e) { console.error(e); }
}
invoke("autostart", {}).then((on) => ($("#autostart").checked = on)).catch(() => {});
refresh();
setInterval(refresh, 1000);
window.addEventListener("resize", () => render());
