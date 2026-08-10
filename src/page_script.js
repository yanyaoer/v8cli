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
