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
  ["input", "Share Input", "Use this PC's keyboard & mouse"],
  ["extend", "Extend", "Extra monitor"],
  ["mirror", "Duplicate", "Same screen on both"],
  ["off", "Off", "Disconnect"],
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

function status(d) {
  if (d.session) return d.session.origin ? `${LABEL[d.session.mode]} · ${d.session.status}` : d.session.status;
  if (busy[d.fp]) return busy[d.fp] === "off" ? "Disconnecting…" : `Connecting · ${LABEL[busy[d.fp]]}…`;
  if (!d.online) return d.trusted ? "Offline · trusted" : "Offline";
  if (d.trusted) return "Ready · trusted";
  if (d.once) return "Ready · allowed this session";
  return "Not paired yet";
}

// Something stuck? Disconnect and connect again with one click.
const reconnectBtn = (fp) => `<div class="acts1"><button class="btn" data-reconnect="${esc(fp)}">Reconnect</button></div>`;

// Sound plays on this computer only, unless the user also wants it on the device showing the screen.
const soundToggle = (key, name) => `<label class="snd"><input type="checkbox" data-sound="${esc(key)}" ${(S.sound || {})[key] ? "checked" : ""}/> Also play sound on ${esc(name)}</label>`;
// Pointer speed while the mouse is on this device (Windows only; bekdesk switches it as the mouse crosses over).
const speedSlider = (key, name) => {
  if (S.me.os !== "windows") return "";
  const v = (S.pointer || {})[key] || 1;
  return `<label class="spd">Mouse speed on ${esc(name)}<input type="range" min="0.5" max="3" step="0.25" value="${v}" data-speed="${esc(key)}" /><b>${v.toFixed(2).replace(/0$/, "")}×</b></label>`;
};

const has = (engine) => !!(S.engines && S.engines[engine] && S.engines[engine].version);

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
    else if (ses && !ses.origin && ses.mode === "input") pill = ["live", `Controlled by ${d.name}`];
    else if (ses && (ses.mode === "input" || ses.streaming || !ses.origin)) pill = ["live", `Connected · ${LABEL[ses.mode]}`];
    else if (ses) pill = ["wait", "Connecting…"];
    else if (!d.online) pill = ["off", "Offline"];
    else pill = ["ready", d.trusted || d.once ? "Ready" : "Not paired"];
    // What this computer can do (Linux can't share its own screen yet, but can show another computer's).
    const host = has("sunshine") ? "" : "Sharing this computer's screen isn't available on Linux yet; pick Extend or Duplicate on the other computer to show its screen here";
    const no = { input: has("deskflow") ? "" : "Not available on this computer yet", extend: host, mirror: host };
    out.push({
      no,
      icon: iconFor(d.kind, d.os),
      key: d.fp, kind: "pc", online: d.online, name: d.name || "Unknown device", os: d.os, sub: `${osName(d.os)} · ${status(d)}`, pill, mode,
      modes: ["input", "extend", "mirror", "off"], disabled: !d.online || !!busy[d.fp], extendOk: winHost, note: d.note,
      extra: !d.trusted && !d.once && d.online
        ? `<div class="hint">First connection asks for approval on ${esc(d.name)}. Check that both screens show code <b>${esc(d.code)}</b>.</div>`
        : d.online ? (ses && !ses.origin && ses.mode === "input"
            ? `<div class="hint ok">${esc(d.name)}'s keyboard and mouse are controlling this computer.</div><div class="acts1"><button class="btn primary" data-mode="input" data-fp="${esc(d.fp)}">Use this PC's keyboard &amp; mouse instead</button></div>` : "")
          + (mode === "input" ? `<div class="hint">Move your mouse past the edge where you put ${esc(d.name)} in Manage Screens. Your keyboard always types on the computer the mouse is on.</div>` : "")
          + `<div class="hint">Drop files on this card to send them to ${esc(d.name)}. Copied files and text sync while Share Input is on.</div>`
          + (mode === "extend" || mode === "input" ? speedSlider(d.fp, d.name) : "")
          + (mode === "extend" || mode === "mirror" ? soundToggle(d.fp, d.name) : "")
          + (mode !== "off" && !busy[d.fp] ? reconnectBtn(d.fp) : "")
          + (d.trusted || d.once ? `<div class="acts1"><button class="link danger" data-forget-card="${esc(d.fp)}">Forget this computer</button></div>` : "") : "",
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
  const dn = paired.map((d) => d.name).filter(Boolean).join(", ") || "Tablet or phone";
  const mode = busy.tablet || t;
  let pill = ["ready", "Off"], extra = "";
  // Moonlight on Android streams 720p unless told otherwise, and only the device can change that.
  const sharp = `<div class="hint">For a sharp picture: in the app on the tablet open <b>Settings</b>, set <b>Video resolution</b> to <b>Native</b>${t === "extend" ? ` (${esc(S.tablet_res)})` : ""} move <b>Video bitrate</b> to about 40 Mbps, and set <b>Frame rate</b> to 90 on a 120 Hz screen (60 on a 60 Hz one).</div>`;
  const list = paired.map((d) => `<div class="mlrow"><span class="gi">${ICON.tablet}</span><b>${esc(d.name || "Unnamed device")}</b><button class="link" data-act="forget" data-uuid="${esc(d.uuid)}">Remove</button></div>`).join("")
    + (paired.length ? `<div class="hint">The tablet says <b>not authorized (401)</b>, or you removed it here? On the tablet long-press <b>${esc(host)}</b> → <b>Delete PC</b>, wait for it to reappear, tap it and pair again with the PIN.</div>` : "");
  if (t !== "off") {
    if (!ts.running) pill = ["wait", "Starting…"];
    else if (ts.streaming) pill = ["live", `Connected · ${LABEL[t]}`];
    else pill = ["wait", "Not connected"];
    extra = ts.streaming ? `<div class="hint ok">Streaming to ${esc(dn)} now. It switches off by itself when ${esc(dn)} disconnects.</div>`
      : paired.length ? `<div class="hint">Open the app on ${esc(dn)}, tap <b>${esc(host)}</b>, then <b>Desktop</b>.${t === "extend" ? " The extra screen appears only while it's connected." : ""}</div>${sharp}`
      : `<div class="hint">Nothing is paired yet. Open the Moonlight app on the tablet or phone and tap <b>${esc(host)}</b>; it asks for a PIN here.</div>`;
    extra += list;
    if (t === "extend") extra += speedSlider("tablet", dn);
    extra += soundToggle("tablet", dn);
    if (!busy.tablet) extra += reconnectBtn("tablet");
    if (t === "extend") extra += `<label class="resrow">Screen size <input id="tab-res" value="${esc(S.tablet_res)}" size="10" /></label>`;
  } else {
    extra = `<div class="hint">Paired. Choose Extend or Duplicate, then open the app on it. One device streams at a time.</div>` + list;
  }
  out.push({ key: "tablet", kind: "tablet", online: true, name: dn, os: "tablet", sub: t === "off" ? "Paired" : t === "extend" ? "Extra monitor for this PC" : "Showing this PC's screen", pill, mode,
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
  const f = e.target.closest("[data-forget-card]");
  if (f && armed(f, "Press again to forget it")) { await call("forget", { fp: f.dataset.forgetCard }); toast("Forgotten. It asks for the code again next time."); refresh(); }
});
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
  if (e.target.dataset.sound) {
    const key = e.target.dataset.sound, on = e.target.checked;
    await call("set_sound", { key, on });
    S.sound = { ...(S.sound || {}), [key]: on };
    reconnect(key); // the stream picks up the sound setting when it starts
    return;
  }
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
    // Extended, the tablet is the next numbered screen of this PC (same number Identify shows).
    vnum: S.tablet === "extend" ? monitorsOf(S.me.monitors).length + 1 : 0 });
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
    const tag = it.kind === "tablet" ? (it.vnum ? `<span class="num">${it.vnum}</span>` : "") : `<span class="num">${i + 1}${i === 0 ? '<em>Main</em>' : ""}${conn}</span>`;
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
async function endMonitorDrag({ mon, g, moved }) {
  dragging = null;
  mon.classList.remove("dragging");
  if (!moved) return;
  // Snap flush against the nearest edge of another of this PC's screens, aligning edges when close.
  let l = mon.offsetLeft, t = mon.offsetTop, best = null;
  for (const o of g.querySelectorAll(".screen")) {
    if (o === mon) continue;
    const top = Math.abs(t - o.offsetTop) < 20 ? o.offsetTop : t, left = Math.abs(l - o.offsetLeft) < 20 ? o.offsetLeft : l;
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
  if (Math.abs(sx - nx) < 40) nx = sx;
  if (Math.abs(sy - ny) < 40) ny = sy;
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
  // Snap flush to the nearest edge of another device, aligning tops/lefts when close.
  let best = null;
  for (const o of groups.values()) {
    if (o === g) continue;
    const top = Math.abs(gt - o._t) < 30 ? o._t : gt, left = Math.abs(gl - o._l) < 30 ? o._l : gl;
    for (const [x, y] of [[o._l + o._w, top], [o._l - g._w, top], [left, o._t + o._ht], [left, o._t - g._ht]]) {
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
function renderSettings() {
  if (document.activeElement !== $("#name")) $("#name").value = S.me.name;
  const devs = S.devices.filter((d) => d.trusted || d.online);
  setHTML($("#trusted"), devs.map((d) => `<div class="card" data-fp="${esc(d.fp)}"><div class="row2"><div class="ico sm">${iconFor(d.kind, d.os)}</div><div class="grow"><b>${esc(d.name)}</b> <span class="small">${esc(osName(d.os))} · ${d.online ? "online" : "offline"} · ${d.trusted ? "trusted" : d.once ? "allowed this session" : "not paired"}</span></div>
      ${d.trusted ? "" : d.online ? `<button class="btn primary" data-pair="${esc(d.fp)}">Ask to connect</button>` : ""}
      ${d.trusted ? `<button class="btn" data-forget="${esc(d.fp)}">Forget</button>` : ""}<button class="btn danger" data-block="${esc(d.fp)}">Block</button></div>
    ${d.trusted ? `<details class="perm"><summary>Permissions</summary><div class="caps">${["input", "clipboard", "files", "screen"].map((k) => `<label><input type="checkbox" data-cap="${k}" ${d.caps[k] ? "checked" : ""}/> ${{ input: "Control this computer", clipboard: "Clipboard", files: "Send files here", screen: "Show its screen here" }[k]}</label>`).join("")}</div></details>` : ""}</div>`).join("")
    || `<div class="empty">No devices found yet. Open bekdesk on your other computer, on the same Wi-Fi.</div>`);
  setHTML($("#links"), Object.entries(LINKS).filter(([, u]) => u).map(([n, u]) => `<button class="btn" data-url="${esc(u)}">${esc(n)}</button>`).join(""));
  setHTML($("#blocked"), Object.entries(S.blocked).map(([fp, n]) => `<div class="card row2"><div class="grow"><b>${esc(n)}</b> <span class="small">${esc(fp.slice(0, 16))}…</span></div><button class="btn" data-unblock="${esc(fp)}">Unblock</button></div>`).join("")
    || `<div class="empty">Nothing blocked.</div>`);
}
$("#name").addEventListener("change", (e) => call("set_name", { name: e.target.value }));
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
window.addEventListener("resize", () => { render(); moveInd(); });
