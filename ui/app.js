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
  ["input", "Share Input", "One keyboard and mouse, shared clipboard and files"],
  ["extend", "Extend", "Use it as another monitor for this computer"],
  ["mirror", "Mirror", "Show this computer's main screen on it"],
  ["off", "Off", "Independent computers"],
];
const LABEL = { input: "Share Input", extend: "Extend", mirror: "Mirror", off: "Off" };
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
function status(d) {
  if (d.session) return d.session.origin ? `${LABEL[d.session.mode]} · ${d.session.status}` : d.session.status;
  if (busy[d.fp]) return busy[d.fp] === "off" ? "Disconnecting…" : `Connecting · ${LABEL[busy[d.fp]]}…`;
  if (!d.online) return d.trusted ? "Offline · trusted" : "Offline";
  if (d.trusted) return "Available · trusted";
  if (d.once) return "Available · allowed this session";
  return "Available · not paired";
}

function renderDevices() {
  const me = S.me;
  const rows = [];
  const devs = [...S.devices].sort((a, b) => (b.online - a.online) || String(a.name).localeCompare(String(b.name)));
  for (const d of devs) {
    const live = !!d.session;
    const mode = busy[d.fp] || (d.session && d.session.origin ? d.session.mode : d.mode);
    const canExtend = me.os === "windows";
    rows.push(`
      <div class="dev ${openFp === d.fp ? "open" : ""}" data-fp="${esc(d.fp)}">
        <div class="top" data-toggle="${esc(d.fp)}">
          <div class="ico">${ICON[d.os] || ICON.windows}</div>
          <div class="t"><div class="n">${esc(d.name || "Unknown device")}</div><div class="s">${esc(osName(d.os))} · ${esc(status(d))}</div></div>
          ${live ? `<span class="badge live">${LABEL[d.session.mode]}</span>` : ""}
          <span class="dot ${live ? "live" : d.online ? "on" : ""}"></span>
          <span class="chev">›</span>
        </div>
        <div class="opts">
          <div class="seg">${MODES.map(([m, t, sub]) => {
            const dis = !d.online || busy[d.fp] || (m === "extend" && !canExtend);
            const why = m === "extend" && !canExtend ? "Extend from macOS isn't available yet" : sub;
            return `<button data-mode="${m}" data-fp="${esc(d.fp)}" class="${mode === m ? "on" : ""}" ${dis ? "disabled" : ""} title="${esc(why)}"><b>${t}</b><span>${esc(why)}</span></button>`;
          }).join("")}</div>
          ${!d.trusted && !d.once && d.online ? `<div class="hint">First connection asks for approval on ${esc(d.name)}. Check that both screens show code <b>${esc(d.code)}</b>.</div>` : ""}
          ${d.note ? `<div class="note">${esc(d.note)}</div>` : ""}
          ${d.online && (d.trusted || d.once) ? `<div class="hint">Drop files on this card to send them to ${esc(d.name)}. While Share Input is on, copied files and text are synced automatically.</div>` : ""}
        </div>
      </div>`);
  }
  if (!S.devices.length) rows.push(`<div class="card start">
      <div class="n" style="font-weight:600">No other computers found yet</div>
      <ol>
        <li>On your other PC or Mac, install bekdesk (same installer) and open it.</li>
        <li>Make sure both are on the same Wi-Fi or network. If Windows asks, allow bekdesk on <b>private</b> networks.</li>
        <li>It appears here within a few seconds. Click it and choose <b>Share Input</b>, <b>Extend</b> or <b>Mirror</b>.</li>
      </ol>
      <div class="small">This computer is <b>${esc(me.name)}</b>. Searching…</div></div>`);
  $("#device-list").innerHTML = rows.join("");

  const t = S.tablet;
  const winHost = me.os === "windows";
  $("#tablet").innerHTML = `
    <div class="row2"><div class="ico">${ICON.tablet}</div>
      <div class="grow"><div class="n" style="font-weight:600">Android tablet</div>
      <div class="small">There is no bekdesk app for Android yet, so the tablet uses the free <b>Moonlight</b> app as its screen.
      1) Choose Mirror or Extend here. 2) Install Moonlight from the Play Store on the tablet and open it — this computer shows up there.
      3) Tap it; type the 4-digit PIN it shows into the box below and press Pair. 4) Tap <b>Desktop</b>.</div></div></div>
    <div class="row">
      <select id="tab-mode">
        <option value="off" ${t === "off" ? "selected" : ""}>Off</option>
        <option value="mirror" ${t === "mirror" ? "selected" : ""}>Mirror this screen</option>
        ${winHost ? `<option value="extend" ${t === "extend" ? "selected" : ""}>Extend to tablet</option>` : ""}
      </select>
      ${t === "extend" ? `<input id="tab-res" value="${esc(S.tablet_res)}" size="10" title="Tablet resolution" />` : ""}
      <input id="tab-pin" placeholder="PIN" maxlength="4" size="5" ${t === "off" ? "disabled" : ""} />
      <button class="btn" id="tab-pair" ${t === "off" ? "disabled" : ""}>Pair</button>
      <span class="small">${esc(S.tablet_status || "")}</span>
    </div>
    ${S.tablet_note ? `<div class="note">${esc(S.tablet_note)}</div>` : ""}`;
}

$("#device-list").addEventListener("click", async (e) => {
  const m = e.target.closest("[data-mode]");
  if (m) {
    const fp = m.dataset.fp, mode = m.dataset.mode;
    busy[fp] = mode;
    render();
    try { await call("set_mode", { fp, mode }); } finally { delete busy[fp]; refresh(); }
    return;
  }
  const t = e.target.closest("[data-toggle]");
  if (t) { openFp = openFp === t.dataset.toggle ? null : t.dataset.toggle; render(); }
});
$("#tablet").addEventListener("change", (e) => {
  if (e.target.id === "tab-mode" || e.target.id === "tab-res") {
    const res = $("#tab-res") ? $("#tab-res").value : S.tablet_res;
    call("set_tablet", { mode: $("#tab-mode").value, res }).then(refresh);
  }
});
$("#tablet").addEventListener("click", async (e) => {
  if (e.target.id === "tab-pair") {
    await call("tablet_pin", { pin: $("#tab-pin").value });
    toast("Tablet paired");
  }
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

// ---------- workspace ----------
function monitorsOf(s) {
  return String(s || "").split(";").map((m) => m.split(",").map(Number)).filter((v) => v.length === 4 && v[2] > 0);
}
function boxOf(mons) {
  if (!mons.length) return { x0: 0, y0: 0, w: 192, h: 108 };
  const x0 = Math.min(...mons.map((m) => m[0])), y0 = Math.min(...mons.map((m) => m[1]));
  const x1 = Math.max(...mons.map((m) => m[0] + m[2])), y1 = Math.max(...mons.map((m) => m[1] + m[3]));
  return { x0, y0, w: (x1 - x0) / 10, h: (y1 - y0) / 10 };
}

function renderWorkspace() {
  if (dragging) return;
  const c = $("#canvas");
  const items = [{ key: "self", name: S.me.name, mons: monitorsOf(S.me.monitors), me: true }]
    .concat(S.devices.filter((d) => d.trusted || d.once).map((d) => ({ key: d.fp, name: d.name, mons: monitorsOf(d.monitors) })));
  const L = S.layout || {};
  const boxes = items.map((it) => ({ ...it, b: boxOf(it.mons), p: L[it.key] || { x: 0, y: 0 } }));
  const minX = Math.min(...boxes.map((b) => b.p.x)), minY = Math.min(...boxes.map((b) => b.p.y));
  const maxX = Math.max(...boxes.map((b) => b.p.x + b.b.w)), maxY = Math.max(...boxes.map((b) => b.p.y + b.b.h));
  const scale = Math.min(1.2, (c.clientWidth - 80) / Math.max(1, maxX - minX), (c.clientHeight - 80) / Math.max(1, maxY - minY));
  c.dataset.scale = scale; c.dataset.minx = minX; c.dataset.miny = minY;
  c.innerHTML = boxes.map((it) => {
    const left = 40 + (it.p.x - minX) * scale, top = 40 + (it.p.y - minY) * scale;
    const mons = it.mons.map((m) => `<div class="mon" style="left:${((m[0] - it.b.x0) / 10) * scale}px;top:${((m[1] - it.b.y0) / 10) * scale}px;width:${(m[2] / 10) * scale - 2}px;height:${(m[3] / 10) * scale - 2}px"></div>`).join("");
    return `<div class="box ${it.me ? "me" : ""}" data-key="${esc(it.key)}" style="left:${left}px;top:${top}px;width:${it.b.w * scale}px;height:${it.b.h * scale}px">${mons}<span class="lbl">${esc(it.name)}${it.mons.length ? "" : " (monitor size unknown)"}</span></div>`;
  }).join("") || `<div class="empty">Pair a device to arrange it.</div>`;
}

$("#canvas").addEventListener("pointerdown", (e) => {
  const box = e.target.closest(".box");
  if (!box) return;
  box.setPointerCapture(e.pointerId);
  dragging = { box, sx: e.clientX, sy: e.clientY, l: box.offsetLeft, t: box.offsetTop };
  box.classList.add("dragging");
});
$("#canvas").addEventListener("pointermove", (e) => {
  if (!dragging) return;
  dragging.box.style.left = dragging.l + e.clientX - dragging.sx + "px";
  dragging.box.style.top = dragging.t + e.clientY - dragging.sy + "px";
});
$("#canvas").addEventListener("pointerup", async () => {
  if (!dragging) return;
  const c = $("#canvas"), scale = +c.dataset.scale, minX = +c.dataset.minx, minY = +c.dataset.miny;
  const boxes = [...c.querySelectorAll(".box")];
  // Snap the dropped box flush against the nearest edge of another box.
  const d = dragging.box;
  let best = null;
  for (const o of boxes) {
    if (o === d) continue;
    const cands = [
      [o.offsetLeft + o.offsetWidth, d.offsetTop], [o.offsetLeft - d.offsetWidth, d.offsetTop],
      [d.offsetLeft, o.offsetTop + o.offsetHeight], [d.offsetLeft, o.offsetTop - d.offsetHeight],
    ];
    for (const [x, y] of cands) {
      const dist = Math.hypot(x - d.offsetLeft, y - d.offsetTop);
      if (!best || dist < best.dist) best = { x, y, dist };
    }
  }
  if (best && best.dist < 60) { d.style.left = best.x + "px"; d.style.top = best.y + "px"; }
  const layout = {};
  for (const b of boxes) layout[b.dataset.key] = { x: Math.round((b.offsetLeft - 40) / scale + minX), y: Math.round((b.offsetTop - 40) / scale + minY) };
  d.classList.remove("dragging");
  dragging = null;
  await call("save_layout", { layout });
  refresh();
});

// ---------- activity ----------
const fmtBytes = (n) => n > 1e9 ? (n / 1e9).toFixed(2) + " GB" : n > 1e6 ? (n / 1e6).toFixed(1) + " MB" : n > 1e3 ? (n / 1e3).toFixed(0) + " KB" : n + " B";
const fmtTime = (s) => s > 3600 ? `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}m` : s > 60 ? `${Math.floor(s / 60)}m` : `${s}s`;

function renderActivity() {
  const live = S.devices.filter((d) => d.session);
  $("#sessions").innerHTML = live.map((d) => `<div class="card row2"><span class="dot live"></span><div class="grow"><b>${esc(d.name)}</b> · ${LABEL[d.session.mode]} ${d.session.origin ? "(from this computer)" : "(from " + esc(d.name) + ")"}<div class="small">${esc(d.session.status)} · ${esc(d.session.ip)} · ${fmtTime(d.session.uptime)}</div></div></div>`).join("")
    || `<div class="empty">No active connections.</div>`;
  $("#transfers").innerHTML = [...S.transfers].reverse().slice(0, 30).map((t) => `<div class="card"><div class="row2"><div class="grow"><b>${esc(t.label)}</b> <span class="small">${t.outgoing ? "to" : "from"} ${esc(t.peer)} · ${esc(t.state)}</span></div><span class="small">${fmtBytes(t.done)} / ${fmtBytes(t.total)}</span></div><div class="bar"><i style="width:${t.total ? (100 * t.done) / t.total : 100}%"></i></div></div>`).join("")
    || `<div class="empty">No transfers yet.</div>`;
  $("#log").textContent = S.log.map((l) => { const [ts, ...r] = l.split(" "); return new Date(+ts * 1000).toLocaleTimeString() + "  " + r.join(" "); }).join("\n");
}

// ---------- settings ----------
function renderSettings() {
  if (document.activeElement !== $("#name")) $("#name").value = S.me.name;
  const trusted = S.devices.filter((d) => d.trusted);
  $("#trusted").innerHTML = trusted.map((d) => `<div class="card" data-fp="${esc(d.fp)}"><div class="row2"><div class="grow"><b>${esc(d.name)}</b> <span class="small">${esc(osName(d.os))} · ${esc(d.fp.slice(0, 16))}…</span></div><button class="btn danger" data-forget="${esc(d.fp)}">Forget</button></div>
    <div class="caps">${["input", "clipboard", "files", "screen"].map((k) => `<label><input type="checkbox" data-cap="${k}" ${d.caps[k] ? "checked" : ""}/> ${{ input: "Control this computer", clipboard: "Clipboard", files: "Send files here", screen: "Show its screen here" }[k]}</label>`).join("")}</div></div>`).join("")
    || `<div class="empty">No trusted devices yet.</div>`;
  $("#blocked").innerHTML = Object.entries(S.blocked).map(([fp, n]) => `<div class="card row2"><div class="grow"><b>${esc(n)}</b> <span class="small">${esc(fp.slice(0, 16))}…</span></div><button class="btn" data-unblock="${esc(fp)}">Unblock</button></div>`).join("")
    || `<div class="empty">Nothing blocked.</div>`;
  $("#engines").innerHTML = Object.entries(S.engines).map(([k, v]) => `<div class="card row2"><div class="grow"><b>${{ deskflow: "Deskflow (input)", sunshine: "Sunshine (display host)", moonlight: "Moonlight (display viewer)", vdd: "Virtual Display Driver" }[k]}</b> ${v.version ? esc(v.version) : "— not for this OS"}</div><span>${v.installed ? "Installed" : v.version ? "Downloads on first use" : ""}</span></div>`).join("");
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
function render() {
  if (!S) return;
  $("#me").innerHTML = `<b>${esc(S.me.name)}</b>${osName(S.me.os)} · ${esc(S.me.fp.slice(0, 8))}`;
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
setInterval(() => { if (!document.querySelector("#tablet input:focus, #tablet select:focus")) refresh(); }, 1000);
window.addEventListener("resize", () => render());
