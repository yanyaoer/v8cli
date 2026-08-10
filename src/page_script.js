(() => {
  if (
    globalThis.__v8cliInstalled &&
    typeof document.tree === "function" &&
    typeof globalThis.__v8cliEval === "function"
  ) return true;
  globalThis.__v8cliInstalled = true;

  const handles = new Map();
  const reverse = new WeakMap();
  let nextHandle = 0;
  let nextTicket = 1;
  const pending = new Map();

  const norm = (value) => String(value ?? "").replace(/\s+/g, " ").trim();
  const trunc = (value, length) => {
    const chars = [...String(value)];
    return chars.length <= length ? chars.join("") : chars.slice(0, length).join("") + "…";
  };
  const quote = (value) => JSON.stringify(String(value));
  const handle = (node) => {
    if (!node) return -1;
    let id = reverse.get(node);
    if (id !== undefined) return id;
    id = nextHandle++;
    reverse.set(node, id);
    handles.set(id, node);
    return id;
  };

  const skip = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "HEAD", "IFRAME"]);
  const textBlocks = new Set([
    "P", "LI", "TD", "TH", "DT", "DD", "BLOCKQUOTE", "PRE", "FIGCAPTION",
    "SUMMARY", "CAPTION", "LABEL", "LEGEND",
  ]);
  const leafSelector = "a[href],button,input,textarea,select,img[alt]";

  function describeLeaf(node) {
    const tag = node.tagName;
    if (tag === "A") {
      const href = node.href || node.getAttribute("href");
      if (!href) return null;
      let text = norm(node.textContent) || node.getAttribute("aria-label") || node.title || "";
      if (!text) text = node.querySelector("img[alt]")?.alt || "";
      return `link ${quote(trunc(text, 80))} -> ${href}`;
    }
    if (tag === "BUTTON") {
      const text = norm(node.textContent) || node.getAttribute("aria-label") || node.value || "";
      return `button ${quote(trunc(text, 80))}`;
    }
    if (tag === "INPUT") {
      let out = `input type=${node.type || "text"}`;
      for (const key of ["name", "placeholder", "value"]) {
        const value = node.getAttribute(key);
        if (value !== null) out += ` ${key}=${quote(trunc(value, 60))}`;
      }
      if (node.required) out += " required";
      return out;
    }
    if (tag === "TEXTAREA") {
      let out = "textarea";
      for (const key of ["name", "placeholder"]) {
        const value = node.getAttribute(key);
        if (value !== null) out += ` ${key}=${quote(trunc(value, 60))}`;
      }
      return out;
    }
    if (tag === "SELECT") {
      const options = [...node.querySelectorAll("option")].slice(0, 8).map((o) => norm(o.textContent));
      return `select${node.name ? ` name=${quote(node.name)}` : ""} [${options.join(" | ")}]`;
    }
    if (tag === "IMG") {
      return node.alt ? `img ${quote(trunc(node.alt, 80))}` : null;
    }
    return null;
  }

  function emitLeaves(root, lines) {
    for (const node of root.querySelectorAll(leafSelector)) {
      const description = describeLeaf(node);
      if (description) lines.push([node, description]);
    }
  }

  function readableText(root) {
    const parts = [];
    const visit = (node) => {
      if (node.nodeType === 3) {
        const text = norm(node.nodeValue);
        if (text) parts.push(text);
        return;
      }
      if (node.nodeType === 1 && skip.has(node.tagName)) return;
      for (const child of node.childNodes || []) visit(child);
    };
    if (root) visit(root);
    return norm(parts.join(" "));
  }

  function walk(root, lines) {
    for (const node of root.children || []) {
      if (skip.has(node.tagName) || node.getAttribute("aria-hidden") === "true") continue;
      const leaf = describeLeaf(node);
      if (leaf) {
        lines.push([node, leaf]);
        continue;
      }
      if (/^H[1-6]$/.test(node.tagName)) {
        const text = norm(node.textContent);
        if (text) lines.push([node, `${node.tagName.toLowerCase()} ${quote(trunc(text, 120))}`]);
        emitLeaves(node, lines);
        continue;
      }
      if (textBlocks.has(node.tagName)) {
        const text = norm(node.textContent);
        const linkText = norm([...node.querySelectorAll("a[href]")].map((a) => a.textContent).join(" "));
        if (text && text !== linkText) lines.push([node, `text ${quote(trunc(text, 200))}`]);
        emitLeaves(node, lines);
        continue;
      }
      if (node.tagName === "FORM") {
        let out = `form method=${(node.method || "get").toUpperCase()}`;
        if (node.action) out += ` action=${node.action}`;
        lines.push([node, out]);
      }
      if (!(node.children || []).length) {
        const text = norm(node.textContent);
        if (text) lines.push([node, `text ${quote(trunc(text, 200))}`]);
        continue;
      }
      walk(node, lines);
    }
  }

  const tree = () => {
    let out = `url: ${location.href}\n`;
    if (document.title) out += `title: ${norm(document.title)}\n`;
    const root = document.body || document.documentElement;
    const lines = [];
    if (root) walk(root, lines);
    if (!lines.length && root) {
      const text = readableText(root);
      if (text) lines.push([root, `text ${quote(trunc(text, 500))}`]);
    }
    const extra = Math.max(0, lines.length - 500);
    for (const [node, description] of lines.slice(0, 500)) {
      out += `[${handle(node)}] ${description}\n`;
    }
    if (extra) out += `... (+${extra} more)\n`;
    return out.trimEnd();
  };

  Object.defineProperties(Element.prototype, {
    handle: { configurable: true, get() { return handle(this); } },
    text: { configurable: true, get() { return norm(this.textContent); } },
    toJSON: { configurable: true, value() {
      const attrs = {};
      for (const attr of this.attributes || []) attrs[attr.name] = attr.value;
      return { tag: this.tagName.toLowerCase(), attrs, text: trunc(norm(this.textContent), 200) };
    } },
  });
  if (globalThis.NodeList && !NodeList.prototype.map) {
    Object.defineProperty(NodeList.prototype, "map", {
      configurable: true,
      value(callback, thisArg) { return Array.from(this).map(callback, thisArg); },
    });
  }

  document.tree = tree;
  document.text = () => readableText(document.body || document.documentElement);
  document.node = (id) => {
    if (id === -1) return document.documentElement;
    if (!Number.isInteger(id) || !handles.has(id)) throw new RangeError(`invalid node handle: ${id}`);
    return handles.get(id);
  };
  globalThis.el = (id) => document.node(id);
  globalThis.openPage = (url) => ({ __v8cliNavigate: String(url) });

  // Only the host can replace the document, so a link click or form submit
  // would otherwise move `location` while leaving the old DOM in place — the
  // caller reads stale content believing it navigated. Capture the intent and
  // let the host perform a real navigation. History API calls are untouched:
  // an SPA route change is not a document fetch.
  // `new FormData(form)` yields nothing on this DOM, so collect the successful
  // controls directly, following the form-submission rules that matter here:
  // named and enabled, checkables only when checked, buttons excluded.
  const formQuery = (form) => {
    const params = new URLSearchParams();
    for (const field of form.querySelectorAll("input, select, textarea")) {
      if (!field.name || field.disabled) continue;
      const type = (field.getAttribute("type") || "").toLowerCase();
      if (["submit", "button", "reset", "image", "file"].includes(type)) continue;
      if ((type === "checkbox" || type === "radio") && !field.checked) continue;
      params.append(field.name, field.value ?? "");
    }
    return params.toString();
  };

  // Holds `{url}` or `{error}`. Throwing from a listener would be swallowed by
  // the dispatcher and the action would look like it succeeded, so failures are
  // recorded and reported by the host instead.
  globalThis.__v8cliNav = null;
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button) return;
    const anchor = event.target?.closest?.("a[href]");
    const href = anchor?.getAttribute("href");
    if (!href || href.startsWith("#") || /^javascript:/i.test(href)) return;
    let resolved;
    try { resolved = new URL(href, location.href); } catch { return; }
    if (!/^https?:$/.test(resolved.protocol)) return;
    if (resolved.href.split("#")[0] === location.href.split("#")[0]) return;
    event.preventDefault();
    globalThis.__v8cliNav = { url: resolved.href };
  }, true);
  document.addEventListener("submit", (event) => {
    if (event.defaultPrevented) return;
    const form = event.target;
    if (!form || form.tagName !== "FORM") return;
    let action;
    try { action = new URL(form.getAttribute("action") || location.href, location.href); }
    catch (error) { globalThis.__v8cliNav = { error: `cannot resolve form action: ${error}` }; return; }
    event.preventDefault();
    if ((form.getAttribute("method") || "get").toLowerCase() !== "get") {
      globalThis.__v8cliNav = { error:
        "POST form submission is unsupported: the host can only navigate with GET. " +
        "Issue the request with fetch(), then openPage() the result if it redirects." };
      return;
    }
    action.search = formQuery(form);
    globalThis.__v8cliNav = { url: action.href };
  }, true);
  globalThis.__v8cliTakeNav = () => {
    const target = globalThis.__v8cliNav;
    globalThis.__v8cliNav = null;
    return target ? JSON.stringify(target) : null;
  };

  // Interaction helpers. `target` is a tree handle, a CSS selector, or an
  // Element, so a caller can act on what `tree()` printed without restating it
  // as a selector.
  const resolve = (target) => {
    if (target && target.nodeType === 1) return target;
    if (typeof target === "number") return document.node(target);
    if (typeof target === "string") {
      const found = document.querySelector(target);
      if (!found) throw new Error(`no element matches ${JSON.stringify(target)}`);
      return found;
    }
    throw new TypeError("target must be a handle, selector or Element");
  };
  globalThis.$ = (selector) => document.querySelector(selector);
  globalThis.$$ = (selector) => Array.from(document.querySelectorAll(selector));

  const fire = (node, type, init) =>
    node.dispatchEvent(new (type === "click" ? MouseEvent : Event)(type, { bubbles: true, cancelable: true, ...init }));

  // Frameworks track the value through the prototype's setter and ignore a
  // plain assignment, so write through it and then announce the change.
  const setValue = (node, value) => {
    const proto = Object.getPrototypeOf(node);
    const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    if (setter) setter.call(node, value);
    else node.value = value;
    fire(node, "input");
    fire(node, "change");
  };

  globalThis.click = (target) => {
    const node = resolve(target);
    node.focus?.();
    node.click();
    return true;
  };
  globalThis.fill = (target, value) => {
    const node = resolve(target);
    node.focus?.();
    setValue(node, String(value));
    return node.value;
  };
  globalThis.type = (target, text) => {
    const node = resolve(target);
    node.focus?.();
    setValue(node, String(node.value ?? "") + String(text));
    return node.value;
  };
  globalThis.select = (target, value) => {
    const node = resolve(target);
    setValue(node, String(value));
    return node.value;
  };
  globalThis.check = (target, on = true) => {
    const node = resolve(target);
    node.checked = !!on;
    fire(node, "input");
    fire(node, "change");
    return node.checked;
  };
  globalThis.submit = (target) => {
    const form = resolve(target ?? "form");
    if (!fire(form, "submit")) return false;
    form.submit?.();
    return true;
  };

  // Resolves once `condition` holds. The host drives the event loop while this
  // promise is pending, so the page keeps running between polls.
  globalThis.waitFor = (condition, timeoutMs = 5000) => {
    const test = typeof condition === "function"
      ? condition
      : () => document.querySelector(String(condition));
    const deadline = Date.now() + timeoutMs;
    return new Promise((resolve, reject) => {
      const poll = () => {
        let hit;
        try { hit = test(); } catch (error) { return reject(error); }
        if (hit) return resolve(hit === true ? true : hit);
        if (Date.now() > deadline) {
          return reject(new Error(`waitFor timed out after ${timeoutMs}ms`));
        }
        setTimeout(poll, 50);
      };
      poll();
    });
  };
  globalThis.waitForText = (needle, timeoutMs = 5000) =>
    waitFor(() => (document.body?.textContent || "").includes(String(needle)), timeoutMs);

  const safeValue = (value) => {
    if (value === undefined) return { undefined: true };
    if (typeof value === "string") return { value };
    try {
      return { value: JSON.parse(JSON.stringify(value)) };
    } catch {
      return { value: String(value) };
    }
  };
  const complete = (value) => {
    if (value && typeof value === "object" && value.__v8cliNavigate) {
      return { navigate: String(value.__v8cliNavigate) };
    }
    return safeValue(value);
  };
  const failure = (error) => ({ error: error?.stack || String(error) });

  globalThis.__v8cliEval = (source) => {
    try {
      const value = (0, eval)(source);
      if (value && typeof value.then === "function") {
        const ticket = nextTicket++;
        pending.set(ticket, { pending: true });
        Promise.resolve(value).then(
          (resolved) => pending.set(ticket, complete(resolved)),
          (error) => pending.set(ticket, failure(error)),
        );
        return { pending: ticket };
      }
      return complete(value);
    } catch (error) {
      return failure(error);
    }
  };
  globalThis.__v8cliTake = (ticket) => pending.get(ticket) || { pending: true };
  return true;
})()
