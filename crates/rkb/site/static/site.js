// The site's only script: theme switch, search, code copy buttons, the phone menu and the one unlock
// for protected lessons. Decryption follows rs-web's rs.crypt: the key is Argon2id (hash-wasm, loaded
// only when a password is used) over the salt, and the content is AES-256-GCM (Web Crypto) with its nonce.
(function () {
  "use strict";
  // The passwords entered in this tab, the encrypted lists they opened, and the opened entries,
  // each tagged with the index of its password in `pw`.
  const UNLOCK = "rkb-unlock";
  const $ = (s, root) => (root || document).querySelector(s);
  const $$ = (s, root) => Array.from((root || document).querySelectorAll(s));
  const arr = (v) => (Array.isArray(v) ? v : []);
  const store = {
    get: (k) => { try { return sessionStorage.getItem(k); } catch (e) { return null; } },
    set: (k, v) => { try { sessionStorage.setItem(k, v); } catch (e) { /* private mode: unlock lasts one page */ } },
    del: (k) => { try { sessionStorage.removeItem(k); } catch (e) { /* nothing stored */ } },
  };
  const TYPES = { pitfall: "Pitfall", recipe: "Recipe", fact: "Fact", decision: "Decision", preference: "Preference" };

  function el(tag, attrs, ...kids) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === "class") e.className = v;
      else e.setAttribute(k, v);
    }
    for (const k of kids) if (k != null) e.append(k);
    return e;
  }

  // Titles in backticks show as code.
  function titleNode(text) {
    const f = document.createDocumentFragment();
    String(text).split(/`([^`]+)`/).forEach((part, i) => f.append(i % 2 ? el("code", null, part) : part));
    return f;
  }

  // Theme
  function theme(t) {
    document.documentElement.dataset.theme = t;
    $$("[data-theme-set]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.themeSet === t)));
  }
  $$("[data-theme-set]").forEach((b) =>
    b.addEventListener("click", () => {
      try { localStorage.setItem("rkb-theme", b.dataset.themeSet); } catch (e) { /* kept for this page only */ }
      theme(b.dataset.themeSet);
    })
  );
  theme(document.documentElement.dataset.theme || "system");

  // Phone menu
  const menu = $("#menu-btn");
  function setMenu(open) {
    document.body.classList.toggle("menu-open", open);
    $("#scrim").hidden = !open;
    if (menu) menu.setAttribute("aria-expanded", String(open));
  }
  if (menu) menu.addEventListener("click", () => setMenu(!document.body.classList.contains("menu-open")));
  $("#scrim").addEventListener("click", () => setMenu(false));

  // Code blocks: a bar with the language and a copy button
  function enhance(root) {
    $$(".prose > pre", root).forEach((pre) => {
      const code = $("code", pre);
      const lang = code && (code.className.match(/language-(\S+)/) || [])[1];
      const button = el("button", { type: "button", class: "btn" }, "Copy");
      button.addEventListener("click", () => {
        navigator.clipboard.writeText(pre.innerText.replace(/\n$/, "")).then(
          () => { button.textContent = "Copied"; setTimeout(() => { button.textContent = "Copy"; }, 1500); },
          () => { button.textContent = "Copy failed"; }
        );
      });
      const box = el("div", { class: "code" }, el("div", { class: "code-bar" }, el("span", null, lang || "text"), button));
      pre.replaceWith(box);
      box.append(pre);
    });
  }
  enhance(document);

  // Search
  const input = $("#search");
  const panel = $("#search-panel");
  let index = null;
  let extra = [];
  let hits = [];
  let selected = -1;
  const off = { type: new Set(), place: new Set() };

  const words = (s) => String(s).toLowerCase().split(/[^\p{L}\p{N}_]+/u).filter(Boolean);
  const where = (e) => (e.place.startsWith("general/") ? "General" : e.place.split("/")[1]);

  function prepare(e) {
    e.tw = words(e.title);
    e.mw = words(arr(e.tags).join(" ") + " " + e.topic);
    e.xw = words(e.text);
    return e;
  }

  async function load() {
    if (!index) {
      const r = await fetch("/search.json");
      index = arr(await r.json()).map(prepare);
    }
    return index.concat(extra);
  }

  // A word in the title scores 5, in the tags or topic 3, in the text 1; a prefix match half that.
  function score(e, q) {
    let total = 0;
    for (const w of q) {
      let best = 0;
      for (const [list, pts] of [[e.tw, 5], [e.mw, 3], [e.xw, 1]]) {
        if (list.includes(w)) best = Math.max(best, pts);
        else if (list.some((x) => x.startsWith(w))) best = Math.max(best, pts / 2);
      }
      if (!best) return 0;
      total += best;
    }
    return total;
  }

  function snippet(text, q) {
    const low = text.toLowerCase();
    let at = -1;
    for (const w of q) {
      const i = low.indexOf(w);
      if (i >= 0 && (at < 0 || i < at)) at = i;
    }
    const start = Math.max(0, at - 70);
    let s = text.slice(start, start + 220);
    if (start > 0) s = "…" + s.replace(/^\S*\s/, "");
    if (start + 220 < text.length) s = s.replace(/\s\S*$/, "") + "…";
    const f = document.createDocumentFragment();
    const re = new RegExp("(" + q.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|") + ")", "gi");
    s.split(re).forEach((part, i) => f.append(i % 2 ? el("mark", null, part) : part));
    return f;
  }

  function filterBox(id, key, counts, label) {
    const box = $(id);
    $$("label", box).forEach((l) => l.remove());
    for (const [name, n] of counts) {
      const cb = el("input", { type: "checkbox" });
      cb.checked = !off[key].has(name);
      cb.addEventListener("change", () => {
        if (cb.checked) off[key].delete(name);
        else off[key].add(name);
        render(false);
      });
      box.append(el("label", null, el("span", null, cb, label(name)), el("span", { class: "count" }, String(n))));
    }
    box.hidden = counts.length === 0;
  }

  let last = [];
  async function run() {
    const raw = input.value.trim();
    if (!raw) return close();
    const q = words(raw);
    const all = await load();
    if (input.value.trim() !== raw) return;
    last = all.map((e) => [score(e, q), e]).filter(([s]) => s > 0)
      .sort((a, b) => b[0] - a[0] || a[1].title.localeCompare(b[1].title)).map(([, e]) => e);
    last.q = q;
    render(true);
  }

  function render(fresh) {
    const q = last.q || [];
    const count = (f) => { const m = new Map(); last.forEach((e) => m.set(f(e), (m.get(f(e)) || 0) + 1)); return [...m]; };
    if (fresh) {
      filterBox("#f-type", "type", count((e) => e.type), (t) => {
        const f = document.createDocumentFragment();
        f.append(el("span", { class: "dot t-" + t }), TYPES[t] || t);
        return f;
      });
      filterBox("#f-place", "place", count(where), (p) => p);
    }
    hits = last.filter((e) => !off.type.has(e.type) && !off.place.has(where(e))).slice(0, 20);
    const shown = last.filter((e) => !off.type.has(e.type) && !off.place.has(where(e))).length;
    $("#results-head").textContent = shown === 0
      ? "No lesson matches “" + input.value.trim() + "”"
      : shown + (shown === 1 ? " lesson matches “" : " lessons match “") + input.value.trim() + "”";
    const list = $("#results");
    list.replaceChildren(...hits.map((e, i) =>
      el("a", { class: "result t-" + e.type, href: e.url, id: "r" + i },
        el("span", { class: "result-meta" }, el("span", { class: "dot" }), TYPES[e.type] || e.type,
          el("span", { "aria-hidden": "true" }, "·"), el("span", { class: "mono" }, e.place),
          e.locked ? el("span", null, "· protected") : null),
        el("span", { class: "result-title" }, titleNode(e.title)),
        el("span", { class: "result-snip" }, snippet(e.text, q)))
    ));
    selected = -1;
    panel.hidden = false;
    $("#search-hint").textContent = shown + (shown === 1 ? " result" : " results");
  }

  function close() {
    panel.hidden = true;
    $("#search-hint").textContent = "/";
    last = [];
  }

  function select(i) {
    if (!hits.length) return;
    selected = (i + hits.length) % hits.length;
    $$(".result").forEach((r, j) => r.classList.toggle("selected", j === selected));
    $("#r" + selected).scrollIntoView({ block: "nearest" });
  }

  let timer;
  input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(run, 80); });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); select(selected + 1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); select(selected - 1); }
    else if (e.key === "Enter" && hits.length) { location.href = hits[Math.max(0, selected)].url; }
    else if (e.key === "Escape") { input.value = ""; close(); input.blur(); document.body.classList.remove("searching"); }
  });
  input.addEventListener("focus", () => { if (input.value.trim()) run(); });
  document.addEventListener("keydown", (e) => {
    const t = e.target;
    const typing = t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName);
    if (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      document.body.classList.add("searching");
      input.focus();
    } else if (e.key === "Escape" && !panel.hidden) {
      close();
    }
  });
  const searchBtn = $("#search-btn");
  if (searchBtn) searchBtn.addEventListener("click", () => {
    const on = document.body.classList.toggle("searching");
    if (on) input.focus(); else close();
  });

  // Protected lessons
  const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
  let argon = null;
  function hashwasm() {
    argon = argon || new Promise((ok, fail) => {
      const s = el("script", { src: "/static/argon2.umd.min.js" });
      s.onload = () => ok(window.hashwasm);
      s.onerror = () => fail(new Error("could not load the Argon2 script"));
      document.head.append(s);
    });
    return argon;
  }

  async function decrypt(password, salt, nonce, data) {
    const hw = await hashwasm();
    const raw = await hw.argon2id({
      password, salt: b64(salt), parallelism: 1, iterations: 3, memorySize: 65536, hashLength: 32, outputType: "binary",
    });
    const key = await crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["decrypt"]);
    const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: b64(nonce) }, key, b64(data));
    return new TextDecoder().decode(plain);
  }

  function groupsOf(list) {
    const groups = new Map();
    for (const e of list) {
      const name = e.project || e.system || e.topic;
      if (!groups.has(name)) groups.set(name, []);
      groups.get(name).push(e);
    }
    for (const g of groups.values()) g.sort((a, b) => a.title.localeCompare(b.title));
    return [...groups].sort((a, b) => a[0].localeCompare(b[0]));
  }

  const baseCount = Number(input.placeholder.replace(/\D/g, ""));
  const total = Number(document.body.dataset.protected || 0);

  function fill(list) {
    extra = list.map((e) => prepare(Object.assign({ locked: true }, e)));
    document.body.classList.add("is-unlocked");
    $("#unlocked").hidden = false;
    input.placeholder = "Search " + (baseCount + list.length) + " lessons, protected included";
    const note = $("#search-note");
    if (note) note.textContent = "Search runs in your browser over the published lessons and the unlocked protected lessons.";
    const locked = total - list.length;
    const lockedRow = () => el("a", { class: "tree-row", href: "/protected/" }, el("span", null, "Locked"), el("span", { class: "count" }, String(locked)));
    const tree = $("#protected-tree");
    if (tree) {
      tree.replaceChildren(...groupsOf(list).map(([name, items]) => {
        const d = el("details", null,
          el("summary", null, el("span", { class: "name" }, name), el("span", { class: "count" }, String(items.length))),
          ...items.map((e) => {
            const a = el("a", { class: "leaf", href: e.url }, titleNode(e.title));
            if (e.url === location.pathname) a.setAttribute("aria-current", "page");
            return a;
          }));
        d.open = items.some((e) => e.url === location.pathname);
        return d;
      }));
      if (locked > 0) tree.append(lockedRow());
    }
    const box = $("#protected-list");
    if (box) {
      $("#unlock").hidden = locked <= 0;
      if (locked > 0) {
        $("#unlock h1").textContent = locked + " protected lessons still locked";
        $("#unlock .lead").textContent = "Another password opens more lessons. The ones you opened stay open until you close this tab.";
      }
      box.hidden = false;
      $("h1", box).textContent = list.length + (locked > 0 ? " protected lessons unlocked" : " protected lessons");
      $("#protected-groups").replaceChildren(...groupsOf(list).map(([name, items]) =>
        el("section", null, el("h2", { class: "mono" }, name),
          el("div", { class: "rows" }, ...items.map((e) =>
            el("a", { class: "row t-" + e.type, href: e.url },
              el("span", { class: "dot", role: "img", "aria-label": TYPES[e.type] || e.type }),
              el("span", { class: "row-title" }, titleNode(e.title)),
              el("span", { class: "mono muted" }, e.topic), el("span", { class: "muted date" }, TYPES[e.type] || e.type)))))));
    }
  }

  function loadState() {
    try {
      const st = JSON.parse(store.get(UNLOCK) || "null");
      if (st && Array.isArray(st.pw) && Array.isArray(st.opened) && Array.isArray(st.list)) return st;
    } catch (e) { /* start over */ }
    return { pw: [], opened: [], list: [] };
  }
  let state = loadState();

  // The password of this protected page, when an opened list holds it.
  function pagePassword() {
    const e = state.list.find((x) => x.url === location.pathname);
    return e ? state.pw[e.k] : null;
  }

  async function openPage(password) {
    const block = $(".encrypted-content");
    if (!block) return;
    const html = await decrypt(password, block.dataset.salt, block.dataset.nonce, block.dataset.encrypted);
    const page = $("#protected-page");
    page.innerHTML = html;
    const h1 = $("h1", page);
    if (h1) document.title = h1.textContent + " | " + document.title.split(" | ").slice(1).join(" | ");
    enhance(page);
    if (location.hash) {
      const target = document.getElementById(location.hash.slice(1));
      if (target) target.scrollIntoView();
    }
  }

  // Tries the password on every list it has not opened yet. Each list has its own salt, so each
  // try is one Argon2 run.
  async function unlock(password) {
    const r = await fetch("/static/protected.json");
    const boxes = arr(await r.json());
    const k = state.pw.includes(password) ? state.pw.indexOf(password) : state.pw.length;
    let opened = 0;
    for (let i = 0; i < boxes.length; i++) {
      if (state.opened.includes(i)) continue;
      let list;
      try {
        list = arr(JSON.parse(await decrypt(password, boxes[i].salt, boxes[i].nonce, boxes[i].ciphertext)));
      } catch (e) {
        continue;
      }
      state.opened.push(i);
      state.list.push(...list.map((e) => Object.assign({}, e, { k })));
      opened++;
    }
    if (!opened) throw new Error("wrong password");
    if (k === state.pw.length) state.pw.push(password);
    store.set(UNLOCK, JSON.stringify(state));
    fill(state.list);
    if ($(".encrypted-content")) {
      const pw = pagePassword();
      if (!pw) {
        msg.textContent = "Unlocked " + opened + (opened === 1 ? " list" : " lists") + ", but this password does not open this page.";
        return;
      }
      await openPage(pw);
    }
  }

  const form = $("#unlock-form");
  const msg = $("#unlock-msg");
  if (form) {
    if (!window.crypto || !crypto.subtle || !window.WebAssembly) {
      msg.textContent = "This browser cannot decrypt: it needs Web Crypto (https or localhost) and WebAssembly.";
    }
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const button = $("button", form);
      const field = $("#pw", form);
      button.disabled = true;
      msg.textContent = "Decrypting…";
      try {
        await unlock(field.value);
        if (!$(".encrypted-content") && msg.textContent === "Decrypting…") msg.textContent = "Unlocked.";
        field.value = "";
      } catch (err) {
        msg.textContent = "Wrong password, or it opens nothing new.";
        field.select();
      } finally {
        button.disabled = false;
      }
    });
  }
  $("#lock-btn").addEventListener("click", () => {
    store.del(UNLOCK);
    location.reload();
  });

  if (state.list.length) {
    fill(state.list);
    const pw = pagePassword();
    if ($(".encrypted-content") && pw) {
      $("#unlock").hidden = true;
      openPage(pw).catch(() => {
        store.del(UNLOCK);
        state = loadState();
        $("#unlock").hidden = false;
      });
    }
  }
})();
