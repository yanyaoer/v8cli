mod dom;
mod net;

use clap::{Parser, Subcommand, ValueEnum};

const BOOTSTRAP: &str = include_str!("bootstrap.js");

#[derive(Parser)]
#[command(name = "v8cli", version, about = "Agent browser-lite: V8 isolate + DOM + fetch, no Chromium")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Evaluate JS (fetch / parseHTML / DOM APIs available)
    Eval { code: String },
    /// Run a JS file
    Run { file: String },
    /// Fetch a URL, bind it as `document`, then print a view or run JS
    Open {
        url: String,
        /// JS to run against the page (overrides --mode)
        #[arg(long)]
        js: Option<String>,
        /// What to print when --js is absent
        #[arg(long, value_enum, default_value_t = Mode::Tree)]
        mode: Mode,
    },
    /// Persistent session: read JSONL {"js": "..."} from stdin, reply one JSON per line.
    /// Isolate, documents, cookies and globals live across requests.
    Serve,
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Tree,
    Text,
    Html,
}

fn main() {
    let cli = Cli::parse();

    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    let mut isolate = v8::Isolate::new(Default::default());
    v8::scope!(let scope, &mut isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    install_ops(scope, context);
    if let Err(e) = run(scope, BOOTSTRAP) {
        eprintln!("bootstrap: {e}");
        std::process::exit(1);
    }

    let code = match cli.cmd {
        Cmd::Eval { code } => code,
        Cmd::Run { file } => match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("cannot read {file}: {e}");
                std::process::exit(1);
            }
        },
        Cmd::Open { url, js, mode } => {
            let open = format!("__openPage({})", serde_json::json!(url));
            if let Err(e) = run(scope, &open) {
                eprintln!("open: {e}");
                std::process::exit(1);
            }
            js.unwrap_or_else(|| {
                match mode {
                    Mode::Tree => "document.tree()",
                    Mode::Text => "document.text()",
                    Mode::Html => "__rawHtml",
                }
                .to_string()
            })
        }
        Cmd::Serve => {
            serve(scope);
            return;
        }
    };

    match run(scope, &code) {
        Ok(out) => {
            if !out.is_empty() {
                println!("{out}");
            }
        }
        Err(e) => {
            eprintln!("js: {e}");
            std::process::exit(1);
        }
    }
}

fn serve(scope: &mut v8::PinScope) {
    use std::io::{BufRead, Write};
    // keep stdout protocol-clean: console goes to stderr
    let _ = run(scope, "Object.assign(console, { log: console.error, info: console.error });");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        // JSONL {"js": "..."} preferred; a raw JS line also works for manual use
        let js = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v["js"].as_str().map(String::from))
            .unwrap_or(line);
        let resp = match run(scope, &js) {
            Ok(v) => serde_json::json!({ "ok": true, "value": v }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        if writeln!(stdout, "{resp}").and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
}

fn run(scope: &mut v8::PinScope, code: &str) -> Result<String, String> {
    v8::tc_scope!(let tc, scope);
    let src = v8::String::new(tc, code).ok_or("source too large")?;
    let result = v8::Script::compile(tc, src, None).and_then(|s| s.run(tc));
    let Some(mut value) = result else {
        let exc = tc.stack_trace().or_else(|| tc.exception());
        return Err(match exc {
            Some(v) => v.to_rust_string_lossy(tc),
            None => "unknown error".to_string(),
        });
    };
    if let Ok(p) = v8::Local::<v8::Promise>::try_from(value) {
        match p.state() {
            v8::PromiseState::Fulfilled => value = p.result(tc),
            v8::PromiseState::Rejected => {
                let msg = p.result(tc).to_rust_string_lossy(tc);
                return Err(format!("unhandled rejection: {msg}"));
            }
            v8::PromiseState::Pending => {
                return Err("promise still pending (no event loop; APIs are synchronous)".into());
            }
        }
    }
    Ok(format_value(tc, value))
}

fn format_value(scope: &v8::PinScope, v: v8::Local<v8::Value>) -> String {
    if v.is_undefined() {
        return String::new();
    }
    if v.is_string() {
        return v.to_rust_string_lossy(scope);
    }
    if let Some(s) = v8::json::stringify(scope, v) {
        let r = s.to_rust_string_lossy(scope);
        if r != "undefined" {
            return r;
        }
    }
    v.to_rust_string_lossy(scope)
}

macro_rules! set_fn {
    ($scope:expr, $global:expr, $name:expr, $cb:expr) => {{
        let key = v8::String::new($scope, $name).unwrap();
        let f = v8::Function::new($scope, $cb).unwrap();
        $global.set($scope, key.into(), f.into());
    }};
}

fn install_ops(scope: &mut v8::PinScope, context: v8::Local<v8::Context>) {
    let global = context.global(scope);
    set_fn!(scope, global, "__host_print", cb_print);
    set_fn!(scope, global, "__host_fetch", cb_fetch);
    set_fn!(scope, global, "__host_parse", cb_parse);
    set_fn!(scope, global, "__host_query", cb_query);
    set_fn!(scope, global, "__host_query_all", cb_query_all);
    set_fn!(scope, global, "__host_node", cb_node);
    set_fn!(scope, global, "__host_text", cb_text);
    set_fn!(scope, global, "__host_inner_html", cb_inner_html);
    set_fn!(scope, global, "__host_doc_text", cb_doc_text);
    set_fn!(scope, global, "__host_tree", cb_tree);
    set_fn!(scope, global, "__host_resolve", cb_resolve);
}

fn throw(scope: &v8::PinScope, msg: &str) {
    let m = v8::String::new(scope, msg).unwrap();
    let e = v8::Exception::error(scope, m);
    scope.throw_exception(e);
}

fn ret_str(scope: &v8::PinScope, rv: &mut v8::ReturnValue, s: &str) {
    match v8::String::new(scope, s) {
        Some(v) => rv.set(v.into()),
        None => throw(scope, "string too large"),
    }
}

fn arg_doc(scope: &v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Option<usize> {
    let id = args.get(i).int32_value(scope).unwrap_or(-1);
    if id < 0 {
        throw(scope, "invalid document id");
        return None;
    }
    Some(id as usize)
}

fn cb_print(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut _rv: v8::ReturnValue) {
    let level = args.get(0).to_rust_string_lossy(scope);
    let msg = args.get(1).to_rust_string_lossy(scope);
    if level == "log" {
        println!("{msg}");
    } else {
        eprintln!("{msg}");
    }
}

fn cb_fetch(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let req = args.get(0).to_rust_string_lossy(scope);
    let resp = net::fetch(&req);
    ret_str(scope, &mut rv, &resp);
}

fn cb_parse(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let html = args.get(0).to_rust_string_lossy(scope);
    let base = {
        let a = args.get(1);
        if a.is_string() {
            Some(a.to_rust_string_lossy(scope))
        } else {
            None
        }
    };
    let id = dom::parse(&html, base.as_deref());
    rv.set(v8::Number::new(scope, id as f64).into());
}

fn cb_query(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let root = args.get(1).int32_value(scope).unwrap_or(-1);
    let sel = args.get(2).to_rust_string_lossy(scope);
    match dom::query(id, root, &sel) {
        Ok(h) => rv.set(v8::Integer::new(scope, h).into()),
        Err(e) => throw(scope, &e),
    }
}

fn cb_query_all(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let root = args.get(1).int32_value(scope).unwrap_or(-1);
    let sel = args.get(2).to_rust_string_lossy(scope);
    match dom::query_all(id, root, &sel) {
        Ok(json) => ret_str(scope, &mut rv, &json),
        Err(e) => throw(scope, &e),
    }
}

fn cb_node(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let h = args.get(1).int32_value(scope).unwrap_or(-1);
    match dom::node_info(id, h) {
        Ok(json) => ret_str(scope, &mut rv, &json),
        Err(e) => throw(scope, &e),
    }
}

fn cb_text(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let h = args.get(1).int32_value(scope).unwrap_or(-1);
    match dom::text(id, h) {
        Ok(s) => ret_str(scope, &mut rv, &s),
        Err(e) => throw(scope, &e),
    }
}

fn cb_inner_html(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let h = args.get(1).int32_value(scope).unwrap_or(-1);
    match dom::inner_html(id, h) {
        Ok(s) => ret_str(scope, &mut rv, &s),
        Err(e) => throw(scope, &e),
    }
}

fn cb_doc_text(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    match dom::doc_text(id) {
        Ok(s) => ret_str(scope, &mut rv, &s),
        Err(e) => throw(scope, &e),
    }
}

fn cb_resolve(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    let href = args.get(1).to_rust_string_lossy(scope);
    match dom::resolve_url(id, &href) {
        Ok(s) => ret_str(scope, &mut rv, &s),
        Err(e) => throw(scope, &e),
    }
}

fn cb_tree(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let Some(id) = arg_doc(scope, &args, 0) else { return };
    match dom::tree(id) {
        Ok(s) => ret_str(scope, &mut rv, &s),
        Err(e) => throw(scope, &e),
    }
}
