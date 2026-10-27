// Script injecté dans chaque page du navigateur de SupaCode : sélection d'éléments,
// dessin par-dessus la page, journal de la console et outils utilisés par les agents.
(() => {
  if (window.top !== window || window.__supacode) return;

  const post = (message) => {
    try {
      window.ipc.postMessage(JSON.stringify(message));
    } catch (_) {}
  };

  // — Console : les agents peuvent relire les erreurs de la page.
  const logs = [];
  const remember = (level, args) => {
    const text = Array.from(args)
      .map((a) => {
        if (a instanceof Error) return a.stack || String(a);
        if (typeof a === "object") {
          try {
            return JSON.stringify(a);
          } catch (_) {
            return String(a);
          }
        }
        return String(a);
      })
      .join(" ");
    logs.push({ level, text: text.slice(0, 2000), time: Date.now() });
    if (logs.length > 300) logs.shift();
  };
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level];
    console[level] = function (...args) {
      remember(level, args);
      return original.apply(this, args);
    };
  }
  window.addEventListener("error", (e) => remember("error", [e.message + (e.filename ? ` (${e.filename}:${e.lineno})` : "")]));
  window.addEventListener("unhandledrejection", (e) => remember("error", ["Promesse rejetée : ", e.reason]));

  // — Calque au-dessus de la page
  let root, hover, canvas, ctx;
  const ACCENT = "#7C6CF2";
  const DRAW = "#FF4D4F";
  const ensureRoot = () => {
    if (root && root.isConnected) return;
    root = document.createElement("div");
    root.setAttribute("data-supacode", "");
    root.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none;";
    hover = document.createElement("div");
    hover.style.cssText = `position:fixed;display:none;border:2px solid ${ACCENT};background:rgba(124,108,242,.12);border-radius:3px;pointer-events:none;`;
    canvas = document.createElement("canvas");
    canvas.style.cssText = "position:fixed;inset:0;width:100%;height:100%;pointer-events:none;";
    root.append(canvas, hover);
    document.documentElement.append(root);
    ctx = canvas.getContext("2d");
    resize();
  };

  let mode = "browse";
  const picks = []; // { el, box, info }
  const strokes = []; // [{ x, y }] en coordonnées de la page
  let current = null;

  const resize = () => {
    if (!canvas) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = innerWidth * ratio;
    canvas.height = innerHeight * ratio;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    redraw();
  };

  const redraw = () => {
    if (!ctx) return;
    ctx.clearRect(0, 0, innerWidth, innerHeight);
    ctx.lineWidth = 3;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.strokeStyle = DRAW;
    for (const stroke of strokes) {
      ctx.beginPath();
      stroke.forEach((p, i) => (i ? ctx.lineTo(p.x - scrollX, p.y - scrollY) : ctx.moveTo(p.x - scrollX, p.y - scrollY)));
      if (stroke.length === 1) ctx.lineTo(stroke[0].x - scrollX + 0.1, stroke[0].y - scrollY);
      ctx.stroke();
    }
    picks.forEach((pick, i) => place(pick.box, pick.el, String(i + 1)));
  };

  const place = (box, el, label) => {
    const r = el.getBoundingClientRect();
    box.style.left = r.left - 2 + "px";
    box.style.top = r.top - 2 + "px";
    box.style.width = r.width + 4 + "px";
    box.style.height = r.height + 4 + "px";
    if (label !== undefined) box.firstChild.textContent = label;
  };

  addEventListener("scroll", () => requestAnimationFrame(redraw), true);
  addEventListener("resize", resize);

  // — Description d'un élément
  const cssEscape = (s) => (window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, "\\$&"));
  const selectorOf = (el) => {
    if (el.id && document.querySelectorAll("#" + cssEscape(el.id)).length === 1) return "#" + cssEscape(el.id);
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && node !== document.documentElement && parts.length < 6) {
      let part = node.tagName.toLowerCase();
      if (node.id && document.querySelectorAll("#" + cssEscape(node.id)).length === 1) {
        parts.unshift("#" + cssEscape(node.id));
        break;
      }
      const classes = Array.from(node.classList).filter((c) => !/^(hover|active|focus)/.test(c)).slice(0, 2);
      if (classes.length) part += "." + classes.map(cssEscape).join(".");
      const parent = node.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter((c) => c.tagName === node.tagName);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(part);
      node = parent;
    }
    return parts.join(" > ");
  };
  const textOf = (el) => (el.innerText || el.value || el.getAttribute("aria-label") || el.getAttribute("alt") || el.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ");
  const describe = (el) => {
    const r = el.getBoundingClientRect();
    const attributes = {};
    for (const name of ["id", "class", "name", "type", "href", "src", "role", "aria-label", "placeholder", "data-testid"]) {
      const value = el.getAttribute(name);
      if (value) attributes[name] = value.slice(0, 200);
    }
    const style = getComputedStyle(el);
    return {
      selector: selectorOf(el),
      tag: el.tagName.toLowerCase(),
      text: textOf(el).slice(0, 160),
      attributes,
      rect: { x: Math.round(r.left + scrollX), y: Math.round(r.top + scrollY), width: Math.round(r.width), height: Math.round(r.height) },
      style: { color: style.color, background: style.backgroundColor, font: `${style.fontWeight} ${style.fontSize} ${style.fontFamily.split(",")[0]}`, display: style.display },
      html: el.outerHTML.slice(0, 800),
    };
  };
  const label = (info) => {
    const id = info.attributes.id ? "#" + info.attributes.id : "";
    const text = info.text ? ` « ${info.text.slice(0, 40)} »` : "";
    return info.tag + id + text;
  };

  const sendState = () => post({ type: "state", picks: picks.map((p) => label(p.info)), strokes: strokes.length, mode });

  const ours = (el) => el && el.closest && el.closest("[data-supacode]");
  const target = (e) => {
    const el = document.elementFromPoint(e.clientX, e.clientY);
    return el && !ours(el) && el !== document.documentElement && el !== document.body ? el : null;
  };

  // — Mode sélection
  const onMove = (e) => {
    if (mode !== "select") return;
    const el = target(e);
    if (!el) {
      hover.style.display = "none";
      return;
    }
    hover.style.display = "block";
    place(hover, el);
  };
  const swallow = (e) => {
    if (mode !== "select" || ours(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
  };
  const onClick = (e) => {
    if (mode !== "select") return;
    swallow(e);
    const el = target(e);
    if (!el) return;
    const existing = picks.findIndex((p) => p.el === el);
    if (existing >= 0) {
      picks[existing].box.remove();
      picks.splice(existing, 1);
    } else {
      const box = document.createElement("div");
      box.style.cssText = `position:fixed;border:2px solid ${ACCENT};border-radius:3px;pointer-events:none;`;
      const tag = document.createElement("div");
      tag.style.cssText = `position:absolute;top:-11px;left:-11px;min-width:20px;height:20px;padding:0 5px;box-sizing:border-box;border-radius:10px;background:${ACCENT};color:#fff;font:600 11px/20px system-ui,sans-serif;text-align:center;`;
      box.append(tag);
      root.append(box);
      picks.push({ el, box, info: describe(el) });
    }
    redraw();
    sendState();
  };
  for (const type of ["mousedown", "mouseup", "pointerdown", "pointerup", "dblclick", "auxclick", "submit"]) addEventListener(type, swallow, true);
  addEventListener("click", onClick, true);
  addEventListener("mousemove", onMove, true);

  // — Mode dessin
  const point = (e) => ({ x: e.clientX + scrollX, y: e.clientY + scrollY });
  const startStroke = (e) => {
    if (mode !== "draw") return;
    e.preventDefault();
    current = [point(e)];
    strokes.push(current);
    canvas.setPointerCapture && canvas.setPointerCapture(e.pointerId);
    redraw();
  };
  const continueStroke = (e) => {
    if (mode !== "draw" || !current) return;
    current.push(point(e));
    redraw();
  };
  const endStroke = () => {
    if (!current) return;
    current = null;
    sendState();
  };

  addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Escape" && mode !== "browse") {
        setMode("browse");
        sendState();
      }
    },
    true
  );

  const setMode = (next) => {
    ensureRoot();
    mode = next;
    hover.style.display = "none";
    canvas.style.pointerEvents = mode === "draw" ? "auto" : "none";
    canvas.style.cursor = mode === "draw" ? "crosshair" : "";
    document.documentElement.style.cursor = mode === "select" ? "crosshair" : "";
    canvas.onpointerdown = startStroke;
    canvas.onpointermove = continueStroke;
    canvas.onpointerup = endStroke;
    canvas.onpointercancel = endStroke;
  };

  // — Accès pour les agents : éléments interactifs numérotés
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const style = getComputedStyle(el);
    return style.visibility !== "hidden" && style.display !== "none" && style.opacity !== "0";
  };
  const INTERACTIVE = "a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=tab],[role=menuitem],[onclick],[contenteditable=true],[tabindex]:not([tabindex='-1'])";
  let nextRef = 1;
  const find = (where) => {
    if (where && where.ref !== undefined && where.ref !== null) return document.querySelector(`[data-sc-ref="${where.ref}"]`);
    if (where && where.selector) return document.querySelector(where.selector);
    return null;
  };

  window.__supacode = {
    setMode: (next) => {
      setMode(next);
      sendState();
    },
    clear: () => {
      picks.splice(0).forEach((p) => p.box.remove());
      strokes.length = 0;
      redraw();
      sendState();
    },
    hideHover: () => hover && (hover.style.display = "none"),
    details: () => ({ url: location.href, title: document.title, picks: picks.map((p) => p.info), strokes: strokes.length }),
    logs: (since) => logs.filter((l) => !since || l.time > since),
    snapshot: () => {
      const elements = [];
      for (const el of document.querySelectorAll(INTERACTIVE)) {
        if (ours(el) || !visible(el)) continue;
        if (!el.hasAttribute("data-sc-ref")) el.setAttribute("data-sc-ref", String(nextRef++));
        const r = el.getBoundingClientRect();
        const item = { ref: Number(el.getAttribute("data-sc-ref")), tag: el.tagName.toLowerCase(), text: textOf(el).slice(0, 80) };
        for (const name of ["type", "name", "href", "role", "placeholder"]) {
          const value = el.getAttribute(name);
          if (value) item[name] = value.slice(0, 120);
        }
        if ("value" in el && el.value && el.tagName !== "BUTTON") item.value = String(el.value).slice(0, 80);
        if ("checked" in el && (el.type === "checkbox" || el.type === "radio")) item.checked = el.checked;
        if (r.bottom < 0 || r.top > innerHeight) item.offscreen = true;
        elements.push(item);
        if (elements.length >= 250) break;
      }
      return {
        url: location.href,
        title: document.title,
        text: (document.body ? document.body.innerText : "").replace(/\n{3,}/g, "\n\n").slice(0, 6000),
        elements,
      };
    },
    // Centre de l'élément visé, après l'avoir fait défiler à l'écran.
    locate: (where) => {
      const el = find(where);
      if (!el) return { error: "élément introuvable" };
      el.scrollIntoView({ block: "center", inline: "center" });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, tag: el.tagName.toLowerCase(), text: textOf(el).slice(0, 80) };
    },
    focus: (where) => {
      const el = find(where);
      if (!el) return { error: "élément introuvable" };
      el.scrollIntoView({ block: "center" });
      el.focus();
      if (el.select && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) el.select();
      return { tag: el.tagName.toLowerCase() };
    },
    select: (where, value) => {
      const el = find(where);
      if (!el || el.tagName !== "SELECT") return { error: "liste déroulante introuvable" };
      const option = Array.from(el.options).find((o) => o.value === value || o.text.trim() === value);
      if (!option) return { error: "option introuvable : " + value, options: Array.from(el.options).map((o) => o.text.trim()) };
      el.value = option.value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return { value: option.value };
    },
    present: (text, selector) => {
      if (selector && !document.querySelector(selector)) return false;
      if (text && !(document.body && document.body.innerText.includes(text))) return false;
      return document.readyState !== "loading";
    },
  };

  // Le script tourne à la création du document : on attend qu'il existe pour suivre le titre.
  const announce = () => post({ type: "page", url: location.href, title: document.title });
  const ready = () => {
    announce();
    const title = document.querySelector("title");
    if (title) new MutationObserver(announce).observe(title, { childList: true, subtree: true });
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready);
  else ready();
  addEventListener("popstate", announce);
  const push = history.pushState;
  history.pushState = function (...args) {
    const result = push.apply(this, args);
    setTimeout(announce);
    return result;
  };
})();
