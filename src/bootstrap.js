"use strict";
(() => {
  const host = {
    fetch: globalThis.__host_fetch,
    parse: globalThis.__host_parse,
    query: globalThis.__host_query,
    queryAll: globalThis.__host_query_all,
    node: globalThis.__host_node,
    text: globalThis.__host_text,
    innerHtml: globalThis.__host_inner_html,
    docText: globalThis.__host_doc_text,
    tree: globalThis.__host_tree,
    resolve: globalThis.__host_resolve,
    print: globalThis.__host_print,
  };
  for (const k of Object.getOwnPropertyNames(globalThis)) {
    if (k.startsWith("__host_")) delete globalThis[k];
  }

  const trunc = (s, n) => (s.length > n ? s.slice(0, n) + "…" : s);
  const fmt = (a) => {
    if (typeof a === "string") return a;
    if (a instanceof Error) return a.stack ?? String(a);
    try {
      return JSON.stringify(a) ?? String(a);
    } catch {
      return String(a);
    }
  };
  const print = (level) => (...a) => host.print(level, a.map(fmt).join(" "));
  globalThis.console = {
    log: print("log"),
    info: print("log"),
    warn: print("err"),
    error: print("err"),
    debug: print("err"),
  };

  class Element {
    #doc;
    #h;
    constructor(doc, h) {
      this.#doc = doc;
      this.#h = h;
    }
    get handle() {
      return this.#h;
    }
    #info() {
      return JSON.parse(host.node(this.#doc, this.#h));
    }
    get tagName() {
      return this.#info().tag.toUpperCase();
    }
    get attributes() {
      return this.#info().attrs;
    }
    getAttribute(name) {
      const v = this.#info().attrs[name];
      return v === undefined ? null : v;
    }
    get id() {
      return this.getAttribute("id") ?? "";
    }
    get className() {
      return this.getAttribute("class") ?? "";
    }
    get href() {
      const h = this.getAttribute("href");
      return h === null ? null : host.resolve(this.#doc, h);
    }
    get src() {
      const s = this.getAttribute("src");
      return s === null ? null : host.resolve(this.#doc, s);
    }
    get textContent() {
      return host.text(this.#doc, this.#h);
    }
    get text() {
      return this.textContent.replace(/\s+/g, " ").trim();
    }
    get innerHTML() {
      return host.innerHtml(this.#doc, this.#h);
    }
    querySelector(sel) {
      const h = host.query(this.#doc, this.#h, sel);
      return h < 0 ? null : new Element(this.#doc, h);
    }
    querySelectorAll(sel) {
      return JSON.parse(host.queryAll(this.#doc, this.#h, sel)).map(
        (h) => new Element(this.#doc, h),
      );
    }
    toJSON() {
      const i = this.#info();
      return { tag: i.tag, attrs: i.attrs, text: trunc(this.text, 200) };
    }
  }

  class Document {
    #doc;
    constructor(docId) {
      this.#doc = docId;
    }
    querySelector(sel) {
      const h = host.query(this.#doc, -1, sel);
      return h < 0 ? null : new Element(this.#doc, h);
    }
    querySelectorAll(sel) {
      return JSON.parse(host.queryAll(this.#doc, -1, sel)).map(
        (h) => new Element(this.#doc, h),
      );
    }
    get title() {
      return this.querySelector("title")?.text ?? "";
    }
    get body() {
      return this.querySelector("body");
    }
    get documentElement() {
      return this.node(-1);
    }
    node(h) {
      return new Element(this.#doc, h);
    }
    text() {
      return host.docText(this.#doc);
    }
    tree() {
      return host.tree(this.#doc);
    }
  }

  // Synchronous by design: no event loop in the MVP runtime.
  globalThis.fetch = (url, opts = {}) => {
    const req = {
      url,
      method: opts.method ?? "GET",
      headers: opts.headers ?? {},
      body: opts.body ?? null,
    };
    const r = JSON.parse(host.fetch(JSON.stringify(req)));
    if (r.error) throw new Error(`fetch ${url}: ${r.error}`);
    return {
      status: r.status,
      ok: r.status >= 200 && r.status < 300,
      url: r.url,
      headers: r.headers,
      text: () => r.body,
      json: () => JSON.parse(r.body),
      document: () => new Document(host.parse(r.body, r.url)),
    };
  };

  globalThis.parseHTML = (html, baseUrl) =>
    new Document(host.parse(html, baseUrl ?? null));

  const store = new Map();
  globalThis.localStorage = {
    getItem: (k) => (store.has(String(k)) ? store.get(String(k)) : null),
    setItem: (k, v) => void store.set(String(k), String(v)),
    removeItem: (k) => void store.delete(String(k)),
    clear: () => void store.clear(),
    key: (i) => [...store.keys()][i] ?? null,
    get length() {
      return store.size;
    },
  };

  globalThis.el = (h) => globalThis.document.node(h);

  globalThis.__openPage = (url) => {
    const r = globalThis.fetch(url);
    globalThis.document = r.document();
    globalThis.location = { href: r.url };
    globalThis.__rawHtml = r.text();
    return r.status;
  };
  globalThis.openPage = globalThis.__openPage;
})();
