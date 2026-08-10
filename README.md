# v8cli — CLI Browser-Lite with DOM + JavaScript

面向 Agent 的轻量浏览器运行时：可变 DOM、页面 JavaScript、网络与 Cookie，
不启动 Chromium，也不包含像素渲染管线。

实现参考 Cloudflare Kitesurf 的 Engine / PageScript / Outbound 分层。浏览器内核复用
Apache-2.0 的 [Obscura](https://github.com/h4ckf0r0day/obscura)，并固定到已验证的
Git revision；`v8cli` 提供 Agent 语义树、CLI/JSONL 协议以及 Safari/Chrome
Cookie 导入。

## 架构

```text
CLI / JSONL
    │
    ▼
Engine                         会话、导航、settle、Cookie
    │
    ├── PageScript             每个页面的 V8 + 可变 DOM + 事件循环
    │      ├── 页面 <script> / modules / Wasm
    │      ├── fetch / XHR / timers / Promise
    │      └── document.tree() / el(handle)
    │
    └── Outbound               HTTP、CORS、请求头、页面 CookieJar
```

页面完成 `load` 后先进行最长 5 秒的自适应 settle；含脚本的页面还会经过短暂的
观察窗口，再从脚本修改后的实时 DOM 生成语义树。没有维护第二份只读 DOM，树中
的 handle 始终指向当前页面节点。

## 用法

```sh
# 导航、执行页面脚本并输出语义树
v8cli open https://example.com

# 动态 DOM 的可读文本 / 最终 HTML
v8cli open https://example.com --mode text
v8cli open https://example.com --mode html

# 在 settled page 中执行 JavaScript
v8cli open https://example.com --js 'document.querySelector("h1").text'
v8cli open https://example.com --js 'document.querySelectorAll("a").map(a => a.href)'

# Promise 会被事件循环驱动并等待结果
v8cli eval 'fetch("https://api.github.com/repos/denoland/deno").then(r => r.json()).then(x => x.stargazers_count)'
v8cli run script.js

# 导入目标主机可用的浏览器 Cookie，并使用对应浏览器默认 UA
v8cli open https://example.com/account --cookies-from-browser safari
v8cli open https://example.com/account --cookies-from-browser chrome

# 持久页面会话
v8cli serve
```

X 等 CSR 页面会执行其应用脚本后再提取内容：

```sh
v8cli open 'https://x.com/user/status/123' --mode text
```

## serve 协议

stdin 每行接受 `{"js":"..."}`，也接受裸 JavaScript；stdout 每行返回一个 JSON：

```text
> {"js":"1 + 1"}
{"ok":true,"value":"2"}
> {"js":"openPage(\"https://example.com\")"}
{"ok":true,"value":"200"}
> {"js":"document.tree()"}
{"ok":true,"value":"url: https://example.com/\n..."}
> {"js":"Promise.reject(new Error(\"bad\"))"}
{"ok":false,"error":"Error: bad..."}
```

导航会创建新的页面 realm，因此 JS globals 和节点 handle 会失效；Cookie 和会话状态
保留。`openPage()` 在命令边界执行导航，不能在同一段 JS 中立即读取新 document。

## 页面 API

除标准 DOM/Web API 外，注入以下 Agent API：

| API | 说明 |
|-|-|
| `document.tree()` | 当前实时 DOM 的紧凑语义树 |
| `document.text()` | 当前页面的归一化可读文本 |
| `document.node(handle)` / `el(handle)` | 取回语义树节点 |
| `Element.handle` | 获取稳定节点 handle |
| `Element.text` | 归一化 `textContent` |
| `NodeList.map(...)` | Agent 友好的数组映射兼容方法 |
| `openPage(url)` | `serve` 会话中导航，成功返回 `200` |

页面原生 `<script>`、外部脚本、Promise、timer、`fetch` 和 XHR 由 Obscura
PageScript 运行时处理。页面脚本错误会尽量降级，不终止整个 CLI 会话。

## 浏览器 Cookie

`--cookies-from-browser` 使用 Rust crate `cookie-scoop`，只选择目标 URL 可用的
Cookie。父域 Cookie 会保留经过边界校验的 `Domain`，以支持页面访问同站 API
子域；目标主机自身的 Cookie 保持 host-only。`Path`、`Secure` 和 `HttpOnly`
属性也会保留。

- Safari 仅支持 macOS，终端通常需要“完全磁盘访问权限”。
- Chrome 自动选择最近活跃的本地 Profile；macOS 可能弹出
  “Chrome Safe Storage”钥匙串授权提示。
- 私密窗口中仅存在于内存的 Cookie 无法导入。
- UA 根据已安装浏览器版本生成；自定义 UA、TLS 指纹和 Client Hints 不保证一致。

## 设计边界

- 不包含 CSS layout、paint、截图、PDF、视频或 WebGL。
- Web Platform API 覆盖取决于固定版本的 Obscura；不是完整 Chromium。
- 没有 Chromium TLS/HTTP2 指纹，复杂 bot challenge 可能失败。
- settle 与观察窗口是有界的；持续网络、长轮询或很晚发生的 DOM 更新可能看不到。
- `eval` 中原生 `fetch`/`Response` 是标准异步 API，不再是旧版同步 shim。
- 每次进程启动是独立会话；`serve` 可在进程生命周期内保持状态。

## 开发验证

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```
