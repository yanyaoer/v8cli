mod blocklist;
mod net;

use blocklist::Blocklist;
use clap::{Args, Parser, Subcommand, ValueEnum};
use obscura::{Browser as ObscuraBrowser, InterceptResolution, Page};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

const PAGE_SCRIPT: &str = include_str!("page_script.js");
const SETTLE_MS: u64 = 5_000;
/// Budget for one observation round. Obscura returns from `settle` as soon as
/// the page is quiescent, so this is a ceiling, not a delay.
const OBSERVE_MS: u64 = 500;
const OBSERVE_ROUNDS: usize = 4;

/// Counts DOM mutations so the observation loop can ask "did anything change?"
/// without walking the document. Installed as a preload script so it is
/// watching before the page's own scripts run.
const MUTATION_OBSERVER: &str = "\
(() => { \
  if (globalThis.__v8cliMutations !== undefined) return; \
  globalThis.__v8cliMutations = 0; \
  new MutationObserver(records => { globalThis.__v8cliMutations += records.length; }) \
    .observe(document, { subtree: true, childList: true, characterData: true, attributes: true }); \
})()";

#[derive(Parser)]
#[command(
    name = "v8cli",
    version,
    about = "Agent browser-lite: mutable DOM + page JavaScript, no Chromium"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Evaluate JavaScript in a blank browser page
    Eval { code: String },
    /// Run a JavaScript file in a blank browser page
    Run { file: String },
    /// Navigate, execute page scripts, settle, then print a view or run JS
    Open {
        url: String,
        /// Import cookies for this URL from Safari or Chrome and use that browser's User-Agent
        #[arg(long, value_enum)]
        cookies_from_browser: Option<Browser>,
        /// JS to run against the settled page (overrides --mode)
        #[arg(long)]
        js: Option<String>,
        /// What to print when --js is absent
        #[arg(long, value_enum, default_value_t = Mode::Tree)]
        mode: Mode,
        #[command(flatten)]
        filters: FilterArgs,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Persistent page session: read JSONL {"js":"..."} from stdin
    Serve {
        #[command(flatten)]
        filters: FilterArgs,
        #[command(flatten)]
        session: SessionArgs,
    },
}

#[derive(Args)]
struct FilterArgs {
    /// Do not block ad, analytics and telemetry requests
    #[arg(long)]
    no_block: bool,
    /// Drop the built-in blocklist, keeping only --filter-list files
    #[arg(long)]
    no_default_filters: bool,
    /// Additional Adblock Plus-syntax filter list file (repeatable), e.g. a
    /// copy of https://easylist.to/easylist/easyprivacy.txt
    #[arg(long, value_name = "PATH")]
    filter_list: Vec<String>,
}

impl FilterArgs {
    fn build(&self) -> Result<Option<Blocklist>, String> {
        if self.no_block {
            return Ok(None);
        }
        Blocklist::new(&self.filter_list, !self.no_default_filters).map(Some)
    }
}

#[derive(Args)]
struct SessionArgs {
    /// Route requests through a proxy, e.g. http://127.0.0.1:8080
    #[arg(long, value_name = "URL")]
    proxy: Option<String>,
    /// Cookie jar file, read before navigating and written back afterwards so a
    /// login survives across invocations. Created if absent.
    #[arg(long, value_name = "PATH")]
    state: Option<String>,
    /// Extra dwell after the page stops mutating, for content committed by a
    /// late one-shot timer that leaves nothing to wait on
    #[arg(long, value_name = "MS", default_value_t = 0)]
    dwell: u64,
}

#[derive(Default)]
struct EngineConfig {
    user_agent: Option<String>,
    blocklist: Option<Blocklist>,
    dwell_ms: u64,
    proxy: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Tree,
    Text,
    Html,
}

#[derive(Clone, Copy, ValueEnum)]
enum Browser {
    Safari,
    Chrome,
}

struct Engine {
    browser: ObscuraBrowser,
    page: Page,
    /// Shared with the interception task so it can judge first- vs third-party
    /// against the document currently loaded, not the one at startup.
    page_url: Arc<Mutex<String>>,
    dwell_ms: u64,
}

impl Engine {
    async fn new(config: EngineConfig) -> Result<Self, String> {
        let EngineConfig {
            user_agent,
            blocklist,
            dwell_ms,
            proxy,
        } = config;
        let mut builder = ObscuraBrowser::builder();
        if let Some(user_agent) = &user_agent {
            builder = builder.user_agent(user_agent.as_str());
        }
        if let Some(proxy) = &proxy {
            builder = builder.proxy(proxy.as_str());
        }
        let browser = builder.build().map_err(|e| e.to_string())?;
        let mut page = browser.new_page().await.map_err(|e| e.to_string())?;
        page.add_preload_script(MUTATION_OBSERVER);
        let page_url = Arc::new(Mutex::new(String::new()));

        if let Some(blocklist) = blocklist {
            // Two complementary paths: URL patterns reach the parser's
            // <script src> fetches (and the JS runtime's own check), while
            // interception applies the full engine, including community list
            // rules that need request context, to what page JS requests.
            page.set_blocked_urls(blocklist.url_patterns());
            let mut requests = page.enable_interception();
            let task_url = Arc::clone(&page_url);
            tokio::spawn(async move {
                while let Some(request) = requests.recv().await {
                    let source = task_url.lock().map(|u| u.clone()).unwrap_or_default();
                    let deny = blocklist.blocks(&request.url, &source, &request.resource_type);
                    let resolution = if deny {
                        InterceptResolution::Fail {
                            reason: "blocked by v8cli filter list".into(),
                        }
                    } else {
                        InterceptResolution::Continue {
                            url: None,
                            method: None,
                            headers: None,
                            body: None,
                        }
                    };
                    // A closed resolver means the page dropped the request; a
                    // closed channel means the page is gone, so stop.
                    if request.resolver.send(resolution).is_err() && requests.is_closed() {
                        break;
                    }
                }
            });
        }

        Ok(Self {
            browser,
            page,
            page_url,
            dwell_ms,
        })
    }

    /// Missing file is not an error: the first run of a named session creates it.
    fn load_state(&self, path: &str) -> Result<usize, String> {
        let path = std::path::Path::new(path);
        if !path.exists() {
            return Ok(0);
        }
        self.browser
            .cookies()
            .load_from_file(path)
            .map_err(|e| format!("cannot read state {}: {e}", path.display()))
    }

    fn save_state(&self, path: &str) -> Result<(), String> {
        self.browser
            .cookies()
            .save_to_file(std::path::Path::new(path))
            .map_err(|e| format!("cannot write state {path}: {e}"))
    }

    fn add_cookies(&self, target: &Url, cookies: &[String]) -> Result<(), String> {
        let store = self.browser.cookies();
        for cookie in cookies {
            store
                .set(cookie, target.as_str())
                .map_err(|e| format!("cannot import cookie: {e}"))?;
        }
        Ok(())
    }

    async fn blank(&mut self) -> Result<(), String> {
        self.navigate("about:blank").await
    }

    async fn navigate(&mut self, url: &str) -> Result<(), String> {
        // Publish before goto: the page's first subresource requests can reach
        // the interception task while goto is still in flight.
        if let Ok(mut current) = self.page_url.lock() {
            *current = url.to_string();
        }
        self.page.goto(url).await.map_err(|e| e.to_string())?;
        if let Ok(mut current) = self.page_url.lock() {
            *current = self.page.url();
        }
        self.page.settle(SETTLE_MS).await;
        // Frameworks commonly commit their first render just after the page has
        // gone quiet, so one settle can capture the application shell instead of
        // its content. Keep re-entering the event loop while the document is
        // still changing and stop as soon as it holds steady: a static document
        // pays a single short round rather than a fixed delay, and a page that
        // is still filling in gets as many rounds as it needs.
        let mut previous = self.mutation_count();
        for _ in 0..OBSERVE_ROUNDS {
            self.page.settle(OBSERVE_MS).await;
            let current = self.mutation_count();
            if current == previous {
                break;
            }
            previous = current;
        }
        // A lone one-shot timer further out than Obscura's quiescence heuristics
        // leaves no trace to wait on, so recovering it costs real dwell time.
        // Opt-in rather than a tax on every page.
        if self.dwell_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.dwell_ms)).await;
            self.page.settle(OBSERVE_MS).await;
        }
        self.install_page_api()?;
        Ok(())
    }

    fn mutation_count(&mut self) -> u64 {
        self.page
            .evaluate("globalThis.__v8cliMutations || 0")
            .as_u64()
            .unwrap_or(0)
    }

    fn install_page_api(&mut self) -> Result<(), String> {
        let installed = self.page.evaluate(PAGE_SCRIPT);
        if installed == Value::Bool(true) {
            Ok(())
        } else {
            Err("failed to install agent page API".into())
        }
    }

    async fn execute(&mut self, code: &str) -> Result<String, String> {
        let call = format!("__v8cliEval({})", serde_json::json!(code));
        let mut result = self.page.evaluate(&call);

        if let Some(ticket) = result.get("pending").and_then(Value::as_u64) {
            let take = format!("__v8cliTake({ticket})");
            for _ in 0..20 {
                self.page.settle(500).await;
                result = self.page.evaluate(&take);
                if result.get("pending").is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if result.get("pending").is_some() {
                return Err(format!("promise still pending after {SETTLE_MS}ms"));
            }
        }

        if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(error.to_string());
        }
        if let Some(target) = result.get("navigate").and_then(Value::as_str) {
            let target = Url::parse(target)
                .or_else(|_| Url::parse(&self.page.url()).and_then(|base| base.join(target)))
                .map_err(|e| format!("invalid navigation URL: {e}"))?;
            self.navigate(target.as_str()).await?;
            return Ok("200".into());
        }
        // A link click or form submit during this call recorded where the page
        // wanted to go; follow it now so the caller never observes the old
        // document behind a new `location`.
        if let Some(pending) = self.page.evaluate("__v8cliTakeNav()").as_str() {
            let pending: Value =
                serde_json::from_str(pending).map_err(|e| format!("invalid navigation: {e}"))?;
            if let Some(error) = pending.get("error").and_then(Value::as_str) {
                return Err(error.to_string());
            }
            if let Some(target) = pending.get("url").and_then(Value::as_str) {
                let target = target.to_string();
                self.navigate(&target).await?;
                return Ok(target);
            }
        }
        if result.get("undefined") == Some(&Value::Bool(true)) {
            return Ok(String::new());
        }
        match result.get("value") {
            Some(Value::String(value)) => Ok(value.clone()),
            Some(value) => Ok(value.to_string()),
            None => Err(format!("invalid page result: {result}")),
        }
    }

    async fn tree(&mut self) -> Result<String, String> {
        self.execute("document.tree()").await
    }

    async fn text(&mut self) -> Result<String, String> {
        self.execute("document.text()").await
    }

    fn html(&mut self) -> String {
        self.page.content()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run_cli().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run_cli() -> Result<(), String> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Eval { code } => {
            let mut engine = Engine::new(EngineConfig::default()).await?;
            engine.blank().await?;
            print_value(engine.execute(&code).await?);
        }
        Cmd::Run { file } => {
            let code =
                std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            let mut engine = Engine::new(EngineConfig::default()).await?;
            engine.blank().await?;
            print_value(engine.execute(&code).await?);
        }
        Cmd::Open {
            url,
            cookies_from_browser,
            js,
            mode,
            filters,
            session,
        } => {
            let target = Url::parse(&url).map_err(|e| format!("open: invalid URL: {e}"))?;
            let import = match cookies_from_browser {
                Some(browser) => {
                    let browser = match browser {
                        Browser::Safari => net::Browser::Safari,
                        Browser::Chrome => net::Browser::Chrome,
                    };
                    let import = net::import_browser(browser, &target).await?;
                    for warning in &import.warnings {
                        eprintln!("cookies: {warning}");
                    }
                    Some(import)
                }
                None => None,
            };

            let mut engine = Engine::new(EngineConfig {
                user_agent: import.as_ref().map(|i| i.user_agent.clone()),
                blocklist: filters.build()?,
                dwell_ms: session.dwell,
                proxy: session.proxy.clone(),
            })
            .await?;
            if let Some(path) = &session.state {
                engine.load_state(path)?;
            }
            if let Some(import) = &import {
                engine.add_cookies(&target, &import.cookies)?;
            }
            engine
                .navigate(target.as_str())
                .await
                .map_err(|e| format!("open: {e}"))?;

            let output = if let Some(js) = js {
                engine.execute(&js).await.map_err(|e| format!("js: {e}"))?
            } else {
                match mode {
                    Mode::Tree => engine.tree().await?,
                    Mode::Text => engine.text().await?,
                    Mode::Html => engine.html(),
                }
            };
            if let Some(path) = &session.state {
                engine.save_state(path)?;
            }
            print_value(output);
        }
        Cmd::Serve { filters, session } => {
            let mut engine = Engine::new(EngineConfig {
                blocklist: filters.build()?,
                dwell_ms: session.dwell,
                proxy: session.proxy.clone(),
                ..Default::default()
            })
            .await?;
            if let Some(path) = &session.state {
                engine.load_state(path)?;
            }
            engine.blank().await?;
            serve(&mut engine).await;
            if let Some(path) = &session.state {
                engine.save_state(path)?;
            }
        }
    }
    Ok(())
}

async fn serve(engine: &mut Engine) {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let js = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|value| value["js"].as_str().map(String::from))
            .unwrap_or(line);
        let response = match engine.execute(&js).await {
            Ok(value) => serde_json::json!({ "ok": true, "value": value }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        };
        if writeln!(stdout, "{response}")
            .and_then(|_| stdout.flush())
            .is_err()
        {
            break;
        }
    }
}

fn print_value(value: String) {
    use std::io::Write;

    if !value.is_empty() {
        let _ = writeln!(std::io::stdout().lock(), "{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn page_scripts_promises_and_live_handles_work() {
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine
            .navigate(
                "data:text/html,<title>Dynamic</title><p id='x'>before</p><script>document.querySelector('%23x').textContent='after'</script>",
            )
            .await
            .unwrap();

        let tree = engine.tree().await.unwrap();
        assert!(tree.contains("title: Dynamic"));
        assert!(tree.contains("text \"after\""));

        assert_eq!(engine.execute("Promise.resolve(42)").await.unwrap(), "42");
        assert_eq!(
            engine
                .execute("new Promise(resolve => setTimeout(() => resolve(7), 25))")
                .await
                .unwrap(),
            "7"
        );
        engine
            .execute("el(0).textContent = 'changed'")
            .await
            .unwrap();
        assert!(engine.tree().await.unwrap().contains("text \"changed\""));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interaction_helpers_drive_form_controls() {
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine
            .navigate(
                "data:text/html,<input id='i'><input type='checkbox' id='c'><button id='b' \
                 onclick='window.out=i.value+\"/\"+c.checked'>go</button>",
            )
            .await
            .unwrap();

        assert_eq!(engine.execute("fill('#i', 'typed')").await.unwrap(), "typed");
        assert_eq!(engine.execute("check('#c')").await.unwrap(), "true");
        engine.execute("click('#b')").await.unwrap();
        assert_eq!(engine.execute("window.out").await.unwrap(), "typed/true");
        // handles from the tree address the same nodes as selectors do
        engine.tree().await.unwrap();
        assert_eq!(engine.execute("fill(0, 'byhandle')").await.unwrap(), "byhandle");
    }

    /// Serves a page with a link and a GET form; `/next` and `/search` report
    /// what they were reached with. Only http(s) navigations are honoured, so
    /// these cases cannot be expressed with `data:` URLs.
    fn spawn_site() -> String {
        // Obscura refuses private addresses unless told otherwise; these tests
        // deliberately talk to a loopback server so they stay hermetic.
        unsafe { std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1") };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                use std::io::{Read, Write};
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let read = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = if path.starts_with("/next") {
                    "<title>Second</title><p>arrived</p>".to_string()
                } else if path.starts_with("/search") {
                    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
                    format!("<title>Results</title><p>query={query}</p>")
                } else {
                    "<title>First</title><a href=\"/next\">go</a>\
                     <form action=\"/search\"><input name=\"q\"><input name=\"skip\" disabled>\
                     <input type=\"submit\" name=\"btn\" value=\"x\"></form>"
                        .to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        base
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clicking_a_link_replaces_the_document_rather_than_only_location() {
        let base = spawn_site();
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine.navigate(&base).await.unwrap();

        engine.execute("click('a')").await.unwrap();
        // The old document must be gone, not merely shadowed by a new location.
        assert_eq!(engine.execute("document.title").await.unwrap(), "Second");
        assert!(engine.text().await.unwrap().contains("arrived"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_form_submits_the_successful_controls() {
        let base = spawn_site();
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine.navigate(&base).await.unwrap();

        engine.execute("fill('input[name=q]', 'a b')").await.unwrap();
        engine.execute("submit('form')").await.unwrap();

        assert_eq!(engine.execute("document.title").await.unwrap(), "Results");
        let text = engine.text().await.unwrap();
        assert!(text.contains("q=a+b"), "query not submitted: {text}");
        assert!(!text.contains("skip"), "disabled control was submitted: {text}");
        assert!(!text.contains("btn"), "submit button was submitted: {text}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_form_submission_reports_instead_of_silently_doing_nothing() {
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine
            .navigate("data:text/html,<form method='post' action='https://example.com/x'><input name='u'></form>")
            .await
            .unwrap();

        let error = engine.execute("submit('form')").await.unwrap_err();
        assert!(error.contains("POST"), "unexpected error: {error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_for_resolves_once_the_page_commits() {
        let mut engine = Engine::new(EngineConfig::default()).await.unwrap();
        engine
            .navigate(
                "data:text/html,<p id='o'>loading</p><script>setTimeout(() => { \
                 document.getElementById('o').textContent = 'ready' }, 200)</script>",
            )
            .await
            .unwrap();

        let value = engine
            .execute("waitFor(() => $('#o').textContent === 'ready', 4000).then(() => $('#o').text)")
            .await
            .unwrap();
        assert_eq!(value, "ready");
    }
}
