# v8cli — Agent Browser-Lite Runtime

面向 Agent 的轻量网页执行环境:V8 isolate + DOM + fetch,不依赖 Chromium。

核心取舍:Agent 需要的是 `HTML -> DOM -> semantic tree -> action`,
不是 `HTML -> Layout -> Paint -> GPU -> Screenshot -> Vision`。
因此本方案完全砍掉渲染管线(no pixels / no GPU / no compositor)。

## 架构

```
            Agent (CLI / stdin)
                   |
        +---------------------+
        |    Rust supervisor  |
        |  clap / ops / state |
        +---------------------+
          |        |        |
     V8 isolate  DOM 存储   HTTP
     (v8 crate)  (scraper)  (reqwest + cookie jar)
          |        |
     bootstrap.js  html5ever + selectors (Servo 同源)
     (Web API shim)
```

- **V8 isolate**(`v8` crate,即 rusty_v8):只做 JS 执行,宿主注入 `__host_*` ops。
- **DOM 不在 JS 里**:HTML 由 Rust 侧 html5ever 解析,树存在宿主;JS 侧的
  `Document` / `Element` 是句柄(handle)的薄包装,CSS selector 匹配走 Servo 的
  `selectors` crate,天然支持完整选择器语法。
- **网络**:reqwest blocking + 进程内 cookie jar + rustls,同步暴露给 JS。
- **无事件循环**(MVP):fetch 同步返回,不需要 tokio/microtask 泵,启动即执行。

## 用法

```sh
# 语义树(默认):编号即节点句柄,可直接用 el(n) 二次查询
v8cli open https://example.com

# 可读文本 / 原始 HTML
v8cli open https://example.com --mode text
v8cli open https://example.com --mode html

# 对页面执行 JS(document / location / el 已绑定)
v8cli open https://example.com --js 'document.querySelector("h1").text'
v8cli open https://example.com --js 'document.querySelectorAll("a").map(a => a.href)'

# 纯 JS / 脚本文件(fetch、parseHTML 可用)
v8cli eval 'fetch("https://api.github.com/repos/denoland/deno").json().stargazers_count'
v8cli run script.js

# 常驻会话:stdin JSONL,isolate/documents/cookies/globals 跨请求存活
v8cli serve
```

### serve 协议(agent 集成入口)

请求一行一个 `{"js": "..."}`(裸 JS 行也接受),响应一行一个 JSON:

```
> {"js": "openPage(\"https://news.ycombinator.com\")"}
{"ok":true,"value":"200"}
> {"js": "document.tree()"}
{"ok":true,"value":"url: ...\n[0] ..."}
> {"js": "el(13).href"}
{"ok":true,"value":"https://..."}
> {"js": "openPage(el(13).href)"}          # 跳转,document 重新绑定
> {"js": "bad("}
{"ok":false,"error":"SyntaxError: ..."}    # 错误不杀会话
```

- `value` 恒为字符串(对象已 JSON 序列化),`console.*` 全部转到 stderr,stdout 只有协议。
- Agent 侧:spawn 子进程接管 stdin/stdout 即可(observe = `document.tree()`,act = 任意 JS),
  一个 agent 一个进程,天然隔离。

语义树输出示例:

```
url: https://example.com/
title: Example Domain
[0] h1 "Example Domain"
[1] text "This domain is for use in illustrative examples…"
[2] link "More information..." -> https://www.iana.org/domains/example
```

## JS API(isolate 内)

| API | 说明 |
|-|-|
| `fetch(url, {method, headers, body})` | 同步;返回 `{status, ok, url, headers, text(), json(), document()}` |
| `resp.document()` | 把响应体解析为 `Document`(自动带 base url) |
| `parseHTML(html, baseUrl?)` | 解析任意 HTML 字符串 |
| `document` / `location` | `open` 命令下自动绑定 |
| `doc.querySelector(All)` | 完整 CSS selector(Servo selectors) |
| `doc.tree()` / `doc.text()` | 语义树 / 可读文本 |
| `doc.node(h)` / `el(h)` | 由语义树编号取回 Element |
| `Element` | `tagName` `attributes` `getAttribute` `textContent` `text`(压缩空白) `innerHTML` `href`/`src`(已解析为绝对 URL) `querySelector(All)` |
| `console` / `localStorage` | shim(localStorage 进程内) |

## 设计决策

1. **DOM 放宿主而非 JS**:selector 引擎、HTML 容错解析都复用 Servo 生态,
   JS 侧只传句柄,少一份树的内存拷贝,也避免在 JS 里手写 selector 引擎。
2. **同步 fetch**:MVP 不跑页面脚本,Agent 脚本是命令式的,同步 API 对
   LLM 生成代码最友好(无 await 陷阱、无 pending promise)。
3. **语义树编号 = 节点句柄**:`open` 输出的 `[n]` 可直接 `el(n)` 继续查,
   一次抓取内 observe -> act 闭环。
4. **一个进程一个 isolate 一个会话**:`eval`/`run`/`open` 一次调用即一个会话;
   `serve` 把会话变成常驻进程,cookie jar / DOM / JS 全局跨请求存活。

## 已知边界(MVP)

- 不执行页面自带 `<script>`:CSR/SPA 页面拿到的是初始 HTML。数据型 SPA 通常
  直接 `fetch` 其 API 更高效。
- 无事件循环:`setTimeout` / pending promise 不支持。
- 二进制响应按 UTF-8 lossy 处理。
- DOM 只读:无 `innerHTML=` 写、无表单提交语法糖(可用 `fetch` POST 手动提交)。
- cookie / localStorage 不跨进程持久化。

## Roadmap

1. `--session <file>`:cookie + localStorage 落盘,跨(非 serve)调用维持会话。
2. tokio 事件循环 + 真 Promise fetch + `setTimeout`,为执行页面脚本铺路。
4. 可选执行页面 `<script>`(白名单 API 面),覆盖轻度 CSR 页面。
5. 表单语法糖:`form.submit({field: value})` 编译为 fetch。
6. microVM 集成:Rust supervisor + isolate 冷启动 <100ms,per-agent 隔离。

## 对比(实测:macOS arm64,release build)

| 方案 | 单实例内存 | 冷启动 | JS 执行 | 渲染 |
|-|-|-|-|-|
| Chromium + CDP | 200-500MB | 秒级 | 完整 | 有(浪费) |
| **v8cli** | 17MB 空载 | 14ms | isolate 内任意 JS | 无(by design) |
| 纯 HTTP + 解析库 | ~10MB | ~10ms | 无 | 无 |

binary 62MB(V8 静态库占绝对大头);`open + 查询` 端到端 ~0.4s,其中网络占 90%+。
