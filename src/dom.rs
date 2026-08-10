use scraper::{ElementRef, Html, Node, Selector};
use std::cell::RefCell;
use std::sync::LazyLock;
use url::Url;

const SKIP: &[&str] = &["script", "style", "noscript", "template", "svg", "head", "iframe"];
const TEXT_BLOCK: &[&str] = &[
    "p", "li", "td", "th", "dt", "dd", "blockquote", "pre", "figcaption", "summary", "caption",
    "label", "legend",
];

static SEL_TITLE: LazyLock<Selector> = LazyLock::new(|| Selector::parse("title").unwrap());
static SEL_BODY: LazyLock<Selector> = LazyLock::new(|| Selector::parse("body").unwrap());
static SEL_A: LazyLock<Selector> = LazyLock::new(|| Selector::parse("a[href]").unwrap());
static SEL_IMG_ALT: LazyLock<Selector> = LazyLock::new(|| Selector::parse("img[alt]").unwrap());
static SEL_OPTION: LazyLock<Selector> = LazyLock::new(|| Selector::parse("option").unwrap());
static SEL_LEAF: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href], button, input, textarea, select, img[alt]").unwrap());

pub struct Doc {
    html: Html,
    nodes: Vec<ego_tree::NodeId>,
    url: Option<Url>,
}

thread_local! {
    static DOCS: RefCell<Vec<Doc>> = const { RefCell::new(Vec::new()) };
}

pub fn parse(html: &str, base: Option<&str>) -> usize {
    let doc = Doc {
        html: Html::parse_document(html),
        nodes: Vec::new(),
        url: base.and_then(|u| Url::parse(u).ok()),
    };
    DOCS.with(|d| {
        let mut d = d.borrow_mut();
        d.push(doc);
        d.len() - 1
    })
}

fn with_doc<T>(id: usize, f: impl FnOnce(&mut Doc) -> Result<T, String>) -> Result<T, String> {
    DOCS.with(|d| {
        let mut d = d.borrow_mut();
        let doc = d.get_mut(id).ok_or("invalid document id")?;
        f(doc)
    })
}

fn register(doc: &mut Doc, nid: ego_tree::NodeId) -> i32 {
    doc.nodes.push(nid);
    (doc.nodes.len() - 1) as i32
}

/// h == -1 resolves to the document root element.
fn elem<'a>(doc: &'a Doc, h: i32) -> Result<ElementRef<'a>, String> {
    if h < 0 {
        return Ok(doc.html.root_element());
    }
    let nid = *doc
        .nodes
        .get(h as usize)
        .ok_or_else(|| "invalid node handle".to_string())?;
    let node = doc.html.tree.get(nid).ok_or_else(|| "stale node".to_string())?;
    ElementRef::wrap(node).ok_or_else(|| "not an element".to_string())
}

fn parse_selector(sel: &str) -> Result<Selector, String> {
    Selector::parse(sel).map_err(|e| format!("bad selector {sel:?}: {e}"))
}

pub fn query(id: usize, root: i32, sel: &str) -> Result<i32, String> {
    let selector = parse_selector(sel)?;
    with_doc(id, |doc| {
        let found = {
            let root_el = elem(doc, root)?;
            root_el.select(&selector).next().map(|e| e.id())
        };
        Ok(match found {
            Some(nid) => register(doc, nid),
            None => -1,
        })
    })
}

pub fn query_all(id: usize, root: i32, sel: &str) -> Result<String, String> {
    let selector = parse_selector(sel)?;
    with_doc(id, |doc| {
        let found: Vec<ego_tree::NodeId> = {
            let root_el = elem(doc, root)?;
            root_el.select(&selector).map(|e| e.id()).collect()
        };
        let handles: Vec<i32> = found.into_iter().map(|nid| register(doc, nid)).collect();
        serde_json::to_string(&handles).map_err(|e| e.to_string())
    })
}

pub fn node_info(id: usize, h: i32) -> Result<String, String> {
    with_doc(id, |doc| {
        let el = elem(doc, h)?;
        let v = el.value();
        let attrs: serde_json::Map<String, serde_json::Value> = v
            .attrs()
            .map(|(k, val)| (k.to_string(), val.into()))
            .collect();
        Ok(serde_json::json!({ "tag": v.name(), "attrs": attrs }).to_string())
    })
}

pub fn text(id: usize, h: i32) -> Result<String, String> {
    with_doc(id, |doc| Ok(elem(doc, h)?.text().collect::<String>()))
}

pub fn inner_html(id: usize, h: i32) -> Result<String, String> {
    with_doc(id, |doc| Ok(elem(doc, h)?.inner_html()))
}

pub fn resolve_url(id: usize, href: &str) -> Result<String, String> {
    with_doc(id, |doc| Ok(resolve(doc.url.as_ref(), href)))
}

/// Readable text extraction: skips script/style/head, breaks lines at block elements.
pub fn doc_text(id: usize) -> Result<String, String> {
    with_doc(id, |doc| {
        let mut raw = String::new();
        collect_text(*doc.html.root_element(), &mut raw);
        let mut out = String::new();
        let mut blank = 0;
        for line in raw.lines() {
            let l = line.trim();
            if l.is_empty() {
                blank += 1;
                if blank > 1 {
                    continue;
                }
                out.push('\n');
            } else {
                blank = 0;
                out.push_str(l);
                out.push('\n');
            }
        }
        Ok(out.trim().to_string())
    })
}

fn is_block(t: &str) -> bool {
    matches!(
        t,
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside" | "nav"
            | "ul" | "ol" | "li" | "table" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "form" | "blockquote" | "pre" | "hr" | "dl" | "dt" | "dd" | "figure" | "figcaption"
            | "details" | "summary"
    )
}

fn collect_text(node: ego_tree::NodeRef<Node>, out: &mut String) {
    for child in node.children() {
        match child.value() {
            Node::Text(t) => {
                let s = norm(t);
                if !s.is_empty() {
                    if !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') {
                        out.push(' ');
                    }
                    out.push_str(&s);
                }
            }
            Node::Element(e) => {
                let name = e.name();
                if SKIP.contains(&name) {
                    continue;
                }
                if name == "br" {
                    out.push('\n');
                    continue;
                }
                let block = is_block(name);
                if block && !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                collect_text(child, out);
                if block && !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

/// Agent-facing semantic tree: numbered interactive/structural nodes.
/// Printed indices are node handles usable via document.node(h) / el(h).
pub fn tree(id: usize) -> Result<String, String> {
    with_doc(id, |doc| {
        let mut header = String::new();
        if let Some(u) = &doc.url {
            header.push_str(&format!("url: {u}\n"));
        }
        let mut items: Vec<(ego_tree::NodeId, String)> = Vec::new();
        {
            if let Some(t) = doc.html.select(&SEL_TITLE).next() {
                let s = norm(&t.text().collect::<String>());
                if !s.is_empty() {
                    header.push_str(&format!("title: {s}\n"));
                }
            }
            let start = doc
                .html
                .select(&SEL_BODY)
                .next()
                .unwrap_or_else(|| doc.html.root_element());
            walk_tree(start, doc.url.as_ref(), &mut items);
        }
        const MAX: usize = 500;
        let extra = items.len().saturating_sub(MAX);
        items.truncate(MAX);
        let mut out = header;
        for (nid, line) in items {
            let h = register(doc, nid);
            out.push_str(&format!("[{h}] {line}\n"));
        }
        if extra > 0 {
            out.push_str(&format!("... (+{extra} more)\n"));
        }
        Ok(out)
    })
}

fn walk_tree(el: ElementRef, base: Option<&Url>, out: &mut Vec<(ego_tree::NodeId, String)>) {
    for child in el.children() {
        let Some(ce) = ElementRef::wrap(child) else { continue };
        let v = ce.value();
        let name = v.name();
        if SKIP.contains(&name) || v.attr("aria-hidden") == Some("true") {
            continue;
        }

        if let Some(line) = describe_leaf(ce, base) {
            out.push((ce.id(), line));
            continue;
        }

        if name.len() == 2 && name.starts_with('h') && name.as_bytes()[1].is_ascii_digit() {
            let text = norm(&ce.text().collect::<String>());
            if !text.is_empty() {
                out.push((ce.id(), format!("{name} {:?}", trunc(&text, 120))));
            }
            emit_leaves(ce, base, out);
            continue;
        }

        if TEXT_BLOCK.contains(&name) {
            let text = norm(&ce.text().collect::<String>());
            let links_text = norm(&ce.select(&SEL_A).flat_map(|a| a.text()).collect::<String>());
            if !text.is_empty() && text != links_text {
                out.push((ce.id(), format!("text {:?}", trunc(&text, 200))));
            }
            emit_leaves(ce, base, out);
            continue;
        }

        if name == "form" {
            let mut s = format!("form method={}", v.attr("method").unwrap_or("get").to_uppercase());
            if let Some(a) = v.attr("action") {
                s.push_str(&format!(" action={}", resolve(base, a)));
            }
            out.push((ce.id(), s));
            walk_tree(ce, base, out);
            continue;
        }

        walk_tree(ce, base, out);
    }
}

fn emit_leaves(el: ElementRef, base: Option<&Url>, out: &mut Vec<(ego_tree::NodeId, String)>) {
    for m in el.select(&SEL_LEAF) {
        if let Some(line) = describe_leaf(m, base) {
            out.push((m.id(), line));
        }
    }
}

fn describe_leaf(el: ElementRef, base: Option<&Url>) -> Option<String> {
    let v = el.value();
    match v.name() {
        "a" => {
            let href = v.attr("href")?;
            if href.is_empty() {
                return None;
            }
            let mut text = norm(&el.text().collect::<String>());
            if text.is_empty() {
                text = v
                    .attr("aria-label")
                    .or_else(|| v.attr("title"))
                    .unwrap_or("")
                    .to_string();
            }
            if text.is_empty() {
                if let Some(img) = el.select(&SEL_IMG_ALT).next() {
                    text = img.value().attr("alt").unwrap_or("").to_string();
                }
            }
            Some(format!("link {:?} -> {}", trunc(&text, 80), resolve(base, href)))
        }
        "button" => {
            let mut text = norm(&el.text().collect::<String>());
            if text.is_empty() {
                text = v
                    .attr("aria-label")
                    .or_else(|| v.attr("value"))
                    .unwrap_or("")
                    .to_string();
            }
            Some(format!("button {:?}", trunc(&text, 80)))
        }
        "input" => {
            let ty = v.attr("type").unwrap_or("text");
            let mut s = format!("input type={ty}");
            for a in ["name", "placeholder", "value"] {
                if let Some(val) = v.attr(a) {
                    s.push_str(&format!(" {a}={:?}", trunc(val, 60)));
                }
            }
            if v.attr("required").is_some() {
                s.push_str(" required");
            }
            Some(s)
        }
        "textarea" => {
            let mut s = String::from("textarea");
            for a in ["name", "placeholder"] {
                if let Some(val) = v.attr(a) {
                    s.push_str(&format!(" {a}={:?}", trunc(val, 60)));
                }
            }
            Some(s)
        }
        "select" => {
            let opts: Vec<String> = el
                .select(&SEL_OPTION)
                .take(8)
                .map(|o| norm(&o.text().collect::<String>()))
                .collect();
            let mut s = String::from("select");
            if let Some(n) = v.attr("name") {
                s.push_str(&format!(" name={n:?}"));
            }
            s.push_str(&format!(" [{}]", opts.join(" | ")));
            Some(s)
        }
        "img" => {
            let alt = v.attr("alt").unwrap_or("");
            if alt.is_empty() {
                None
            } else {
                Some(format!("img {:?}", trunc(alt, 80)))
            }
        }
        _ => None,
    }
}

fn resolve(base: Option<&Url>, href: &str) -> String {
    match base {
        Some(b) => b
            .join(href)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| href.to_string()),
        None => href.to_string(),
    }
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}
