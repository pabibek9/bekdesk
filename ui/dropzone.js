// A small target on the screen edge facing another computer: drop files or images here to send them there.
const q = new URLSearchParams(location.search);
const fp = q.get("fp"), name = q.get("name") || "the other computer", side = q.get("side") || "right";
const z = document.getElementById("z"), t = document.getElementById("t");
document.getElementById("a").textContent = { left: "←", right: "→", up: "↑", down: "↓" }[side] || "→";
z.classList.toggle("v", side === "left" || side === "right");
const idle = () => { z.className = side === "left" || side === "right" ? "v" : ""; t.textContent = `Drop to send to ${name}`; };
idle();
const win = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
win.onDragDropEvent(async (e) => {
  const p = e.payload;
  if (p.type === "enter" || p.type === "over") z.classList.add("hot");
  else if (p.type === "leave") z.classList.remove("hot");
  else if (p.type === "drop" && p.paths && p.paths.length) {
    t.textContent = "Sending…";
    try {
      await window.__TAURI__.core.invoke("drop_files", { fp, paths: p.paths });
      z.className += " ok";
      t.textContent = `Sent. Paste on ${name}`;
    } catch (err) {
      z.className += " err";
      t.textContent = "Couldn't send";
    }
    setTimeout(idle, 3500);
  }
});
