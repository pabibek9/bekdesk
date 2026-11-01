// Light, dark or follow the system ("auto"); runs before the page draws so it never flashes the wrong colours.
(() => {
  const q = matchMedia("(prefers-color-scheme: dark)");
  const pref = () => { try { return localStorage.getItem("theme") || "auto"; } catch (_) { return "auto"; } };
  const apply = () => { const p = pref(); document.documentElement.classList.toggle("dark", p === "dark" || (p === "auto" && q.matches)); };
  window.setTheme = (p) => { try { localStorage.setItem("theme", p); } catch (_) {} apply(); };
  window.themePref = pref;
  q.addEventListener("change", apply);
  apply();
})();
