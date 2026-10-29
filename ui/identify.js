const n = new URLSearchParams(location.search).get("n") || "?";
const el = document.getElementById("n");
el.textContent = n;
if (n.length > 2) el.style.fontSize = "56px";
