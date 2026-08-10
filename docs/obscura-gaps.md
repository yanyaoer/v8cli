# Obscura facade 未暴露能力清单

`obscura-browser::Page` 有 67 个公开方法，`obscura::Page`(我们依赖的 facade)
只转出 14 个。能力大多已经实现且被 CDP 层用着，只是没导出——到目前为止我们需要
的三个都是这种情况。遇到"Obscura 做不到"时先查本表，多半只是缺一个透传。

生成方式：

```sh
# 在 obscura 仓库里比对两侧 impl Page 的 pub fn
rg -n 'pub (async )?fn' crates/obscura-browser/src/page.rs
rg -n 'pub (async )?fn' crates/obscura/src/page.rs
```

## 已补(本地 fork，分支 `feat/subresource-filter`)

| 方法 | 暴露为 | 用途 |
|-|-|-|
| `set_blocked_urls` | 同名 | 拦截 parser 发现的 `<script src>`，广告过滤靠它 |
| `navigate_with_wait_post` | `goto_with_body` | POST 表单导航(登录) |
| — | — | 另修了 `wait_for_selector` 不驱动事件循环、`FormData(form)` 构造函数忽略参数 |

## 需要时再补(与 agent 相关)

| 方法 | 用途 | 备注 |
|-|-|-|
| `navigate_with_wait` | 指定 `WaitUntil` 策略 | 想要 `domcontentloaded` 而非 `load` 时 |
| `set_navigation_timeout` / `navigation_timeout` | 单页导航超时 | 目前只能用 `OBSCURA_NAV_TIMEOUT_MS` 环境变量 |
| `fetched_urls` / `get_response_body` / `take_response_body_raw` | 看页面发过哪些请求、取响应体 | 对标 agent-browser 的 `network requests`；也是"直接截获 SPA 的 API JSON"那条路 |
| `settle_for_duration` | 严格按时长 settle | 比 `--dwell` 更精确 |
| `evaluate_with_timeout` | 带超时的求值 | 防死循环脚本卡住会话 |
| `set_viewport` | CSS 视口 | 响应式脚本会读它，即使不渲染也影响 DOM |
| `push_history` / `set_history_index` | 前进/后退 | agent-browser 有，我们没有 |
| `dom` / `with_dom` | 宿主侧直接访问 DOM 树 | 绕开 JS 做批量抽取可能更快 |
| `set_preload_scripts` / `execute_preload_script` | 批量设置 preload | 现在用 `add_preload_script` 够了 |
| `release_object` / `release_object_group` | 释放 JS 对象句柄 | 长会话内存增长时再说 |
| `navigate_blank` / `has_js` / `url_string` | 杂项 | `url_string` 已由 facade 的 `url()` 覆盖 |

## 需要 `render` feature(重新评估过)

`obscura` 的 `[features]` 里**有 `render`**(`render = ["api", "obscura-browser/render"]`)，
所以截图并非架构上不可能——我此前说"追截图等于把 Chromium 请回来"是不准确的。
代价是 `obscura-render` 那 66,530 行 Rust 进入构建(体积与编译时间)，而且这些方法
**在 facade 上完全没导出**，即使开了 feature 也要先补透传。

| 方法 |
|-|
| `screenshot` / `screenshot_region` / `screenshot_with_animation_sample` / `screenshot_at_animation_time` / `screenshot_region_at_animation_time` / `screenshot_region_with_animation_sample` / `screenshot_scroll_offset` |
| `prepare_screenshot_resources` / `prepared_content_size` / `prepared_content_size_at_animation_time` / `prepared_content_size_with_animation_sample` / `prepared_has_active_css_animations` |
| `apply_device_metrics_override` / `clear_device_metrics_override` / `set_device_scale_factor` / `set_screen_size_override` / `set_default_background_color_override` / `live_animation_sample` |

## 内部管道，不该由我们调用

`*_for_cdp` 系列、`set_intercept_tx`、`enable_intercept`、`take_pending_binding_calls`、
`sync_js_network_events`、`alias_response_body`、`clear_response_bodies`、
`isolate_handle`、`suspend_js` / `resume_js`、`cancel_v8_termination`、
`run_autonomous_event_loop_turn`、`process_pending_navigation`、
`take_pending_navigation`、`new`。

## 为什么不做统一的"反射式"逃生口

Rust 没有运行时反射。等价做法是把 `obscura::Page` 的 `pub(crate) inner: InnerPage`
改成公开(或加 `inner_mut()`)，一次拿到全部 67 个方法，省掉逐个透传。

不这么做的理由：那会把 `obscura_browser::Page` 这个内部类型泄进我们的代码，
每次升级 pin 的 rev 破坏面都变大；而且它否定了 facade 存在的意义，上游基本不会接受，
我们就永远得维护 fork。逐个透传虽然要写几行，但每一个都是干净、可独立上游的补丁——
到目前三个补丁都是 5~20 行，成本远低于耦合内部类型的代价。
