# mr-guile-lsp-server

**English** · [中文](#中文)

A Language Server Protocol (LSP) server for **GNU Guile 3.0.11**, written in
Rust with [`tower-lsp`](https://crates.io/crates/tower-lsp), designed for the
[Helix](https://helix-editor.com/) editor. It pairs a small, fast Rust LSP
shell with a Guile REPL subprocess that reuses the mature **Geiser** analysis
engine — reliable completions, documentation, signatures, go-to-definition and
compiler diagnostics without reinventing Guile's macro/module semantics.

---

## 中文

为 **GNU Guile 3.0.11** 打造的 LSP（语言服务器协议）服务器，用 Rust 的
[`tower-lsp`](https://crates.io/crates/tower-lsp) 编写，面向
[Helix](https://helix-editor.com/) 编辑器。它把轻量快速的 Rust LSP 外壳与一个
Guile REPL 子进程结合，复用成熟的 **Geiser** 分析引擎——提供可靠的补全、文档、
签名、定义跳转和编译诊断，无需在 Rust 中重新实现 Guile 的宏/模块语义。

---

# Features · 功能

| LSP method | Source · 来源 | Notes · 说明 |
|---|---|---|
| `textDocument/completion` | Geiser `apropos` | prefix-matched bindings · 前缀匹配的绑定 |
| `textDocument/hover` | Geiser docstrings | user + library symbols · 用户与库符号 |
| `textDocument/signatureHelp` | Geiser `autodoc` | required/optional/key args · 必选/可选/键参数 |
| `textDocument/definition` | Rust structural scan + Geiser `symbol-location` | precise in-file + cross-module · 文件内精确 + 跨模块 |
| `textDocument/documentSymbol` | Rust structural scan | top-level `(define …)` forms · 顶层定义 |
| `textDocument/publishDiagnostics` | `compile-file` warnings + errors | unbound vars, unbalanced parens · 未绑定变量、括号不匹配 |

Positions use **UTF-8** encoding, so Chinese comments/strings line up correctly.
位置使用 **UTF-8** 编码，中文注释/字符串位置准确。

---

# Requirements · 环境要求

- **Rust** (stable; tested on 1.95) — to build the server · 构建服务器
- **GNU Guile 3.0.11** (any 3.0.x works) — the `guile` binary on `PATH` · 作为分析子进程
- No external Guile packages needed (Geiser is vendored under `deps/`) · 无需额外 Guile 包（Geiser 已内置）

Check / 检查：
```bash
rustc --version    # need stable Rust · 需要 Rust 稳定版
guile --version    # need 3.0.x · 需要 3.0.x
```

---

# Installation · 安装

Pick ONE of the two methods below. · 以下两种方式任选其一。

## A. Manual install · 人工安装（给人看）

Build and install the binary · 构建并安装二进制：
```bash
# from the project root · 在项目根目录执行
cargo build --release
# binary: ./target/release/mr-guile-lsp-server

# install to ~/.cargo/bin (already on PATH) · 安装到 PATH
cargo install --path .
```

Verify · 验证：
```bash
mr-guile-lsp-server --help 2>/dev/null; echo "exit: $?"   # should not error · 不应报错
```

**Release the bundled Scheme modules (required, once) · 释放内置 Scheme 模块（必做，仅一次）：**
```bash
mr-guile-lsp-server --extract-deps
# extracts to ~/.cache/mr-guile-lsp/<version>/ and exits · 释放到缓存后退出
```
The server does **not** auto-extract on first use — you must run this once after
install so the Guile subprocess can load the bundled Geiser modules. Re-running
it is harmless (reuses the existing cache). · 服务端**不会**首次使用时自动释放——
安装后必须手动执行一次，Guile 子进程才能加载内置 Geiser 模块。重复执行无害（复用缓存）。

Then configure Helix (see [Helix configuration](#helix-configuration--helix-配置) below).
然后配置 Helix（见下方 [Helix 配置](#helix-configuration--helix-配置)）。

## B. Agent-assisted install · 智能体安装（给智能体看）

Using Claude Code, Codex, zcode, or any coding agent? Copy the block below and
send it to your agent — it will perform the install for you.
正在使用 Claude Code、Codex、zcode 等编程智能体？将下方代码块完整复制并发送给你的智能体，
它会替你完成安装。

> 📋 **Copy everything between the `---` lines · 复制下面两条 `---` 之间的全部内容：**

```
---
请帮我安装 mr-guile-lsp-server 这个 Guile 3.0.11 的 LSP 服务器（Rust + tower-lsp，面向 Helix）。
项目目录：<请在此填写项目根目录的绝对路径，例如 /home/user/mr-guile-lsp-server>

请按以下步骤执行：
1. 进入项目目录，确认存在 Cargo.toml 与 deps/ 目录。
2. 检查环境：运行 `rustc --version`（需要 Rust 稳定版）与 `guile --version`（需要 3.0.x）。
   若缺失，先告知我如何安装，再继续。
3. 运行 `cargo build --release` 构建二进制；若失败，报告错误并修复。
4. 运行 `cargo test` 与 `guile -L deps -L tests/scheme -s tests/scheme/test-helpers.scm`，
   确认全部测试通过（Rust 测试 + Scheme 测试均应 0 失败）。
5. 运行 `cargo install --path .` 把二进制安装到 ~/.cargo/bin（已在 PATH 中）。
6. 【必做】运行 `mr-guile-lsp-server --extract-deps` 释放内置 Scheme 模块到
   ~/.cache/mr-guile-lsp/<版本>/（服务端不会自动释放，必须手动执行一次，否则补全/诊断不可用）。
   确认命令成功退出（退出码 0），并检查 ~/.cache/mr-guile-lsp/<版本>/.extracted 标记存在。
7. 读取项目根的 README.md，按其中的「Helix 配置」一节，把对应配置写入我的
   ~/.config/helix/languages.toml（若文件或目录不存在则创建；若已存在 scheme 语言配置，做合并而非覆盖）。
8. 运行 `bash tests/lsp_smoke.sh ./target/release/mr-guile-lsp-server` 确认握手成功。
9. 完成后告诉我：安装位置、是否已释放模块、是否已配置 Helix、以及如何打开一个 .scm 文件验证。

执行前请先确认计划；若任何步骤失败，停下并说明原因，不要继续后续步骤。
---
```

---

# Uninstallation · 卸载

## A. Manual uninstall · 人工卸载（给人看）

```bash
# 1. remove the installed binary · 删除已安装的二进制
cargo uninstall mr-guile-lsp-server
# (or remove ./target if you only built without installing · 或若仅 build 未 install，删除 ./target)

# 2. remove the Helix config you added · 删除你加入的 Helix 配置
#    edit ~/.config/helix/languages.toml and delete the [language-server.guile-lsp]
#    and the scheme language-servers entry · 编辑该文件，删除 guile-lsp 与 scheme 相关条目

# 3. (optional) clean runtime caches the server wrote · （可选）清理运行时缓存
rm -rf ~/.cache/mr-guile-lsp /tmp/mr-guile-lsp-src-*.scm
```

## B. Agent-assisted uninstall · 智能体卸载（给智能体看）

> 📋 **Copy everything between the `---` lines · 复制下面两条 `---` 之间的全部内容：**

```
---
请帮我卸载 mr-guile-lsp-server 这个 Guile LSP 服务器。

请按以下步骤执行：
1. 如果是用 `cargo install --path .` 安装的，运行 `cargo uninstall mr-guile-lsp-server` 卸载二进制；
   如果只是 `cargo build`、未 install，则删除项目下的 `./target` 目录即可。先确认我用的哪种方式再操作。
2. 编辑我的 ~/.config/helix/languages.toml，删除 `[language-server.guile-lsp]` 段，
   以及 `[[language]] name = "scheme"` 中 `language-servers = ["guile-lsp"]` 这一项
   （如果删除后该 [[language]] 段变空或只剩这一项，可整段删除；若还有其他配置则保留其余内容）。
   操作前先备份该文件，并展示你要做的改动让我确认。
3. 清理运行时缓存：`rm -rf ~/.cache/mr-guile-lsp /tmp/mr-guile-lsp-src-*.scm`
   （前者是释放的内置 Geiser/LSP 模块缓存，后者是每文档待编译副本）。
4. 注意：不要删除项目源码目录本身，除非我明确要求。
5. 完成后告诉我做了哪些改动。

执行前请先说明计划；删除/修改文件前先备份并让我确认。
---
```

---

# Helix configuration · Helix 配置

Add to / 加入 `~/.config/helix/languages.toml`：

```toml
[language-server.guile-lsp]
command = "mr-guile-lsp-server"

[[language]]
name = "scheme"
language-servers = ["guile-lsp"]
```

Restart Helix and open a `.scm` file. Diagnostics appear on open/save/change.
重启 Helix，打开 `.scm` 文件，诊断会在打开/保存/修改时出现。

Logs go to stderr — run Helix with `RUST_LOG=mr_guile_lsp_server=debug helix`
for verbose output.
日志输出到 stderr——用 `RUST_LOG=mr_guile_lsp_server=debug helix` 启动可看详细日志。

---

# How it works · 工作原理

```
Helix ──stdio LSP──► Rust (tower-lsp) ──sentinel pipe──► guile REPL subprocess
                      protocol, docs,            (deps/mr-guile-lsp/lsp-helpers.scm +
                      UTF-8 positions             vendored Geiser modules)
```

- `src/` — the Rust server: LSP handlers, document store, S-expr parser,
  position math, diagnostic builder, REPL client.
  Rust 服务器：LSP 处理器、文档存储、S-expr 解析、位置换算、诊断构建、REPL 客户端。
- `deps/mr-guile-lsp/` — Scheme code embedded into the binary: `lsp-helpers.scm`
  (adapter, ported from `rgherdt/scheme-lsp-server`) + `geiser/` (Geiser's Guile
  modules). See [deps/README.md](deps/README.md) and
  [deps/MODIFICATIONS.md](deps/MODIFICATIONS.md).
  内嵌进二进制的 Scheme 代码：适配层（移植自 scheme-lsp-server）+ Geiser 模块。

At startup the server extracts those embedded Scheme files once to a persistent
cache (`~/.cache/mr-guile-lsp/<version>/`, honors `$XDG_CACHE_HOME` /
`$MR_GUILE_LSP_CACHE_DIR`) and reuses it on later launches.
启动时，server 会把内嵌的 Scheme 文件**一次性释放**到持久缓存
（`~/.cache/mr-guile-lsp/<版本>/`，遵循 `$XDG_CACHE_HOME` / `$MR_GUILE_LSP_CACHE_DIR`），
后续启动直接复用，不再重复释放。

---

# Performance & concurrency · 性能与并发

- **Multi-threaded Rust shell** (tokio multi-thread runtime): the LSP protocol,
  document store (`DashMap`), position/S-expr parsing and diagnostic building
  run in parallel across cores. · Rust 外壳多线程，协议/文档/解析并行。
- **Single Guile REPL** for semantic ops (completion/hover/signature/goto):
  Guile's REPL is inherently single-threaded, so these requests serialize on a
  mutex. Pure-Rust work (e.g. `documentSymbol`) does **not** touch the REPL and
  stays parallel. · 语义操作经单 Guile REPL 串行（互斥）；纯 Rust 计算仍并行。
- **Non-blocking, debounced diagnostics**: `did_change` fires on every keystroke;
  a `DebouncedScheduler` (300 ms quiet period + version coalescing + stale-skip)
  runs `compile-file` in a background task so a slow compile never blocks
  completion/hover. Only the final buffer version after a typing burst is
  compiled. · 诊断非阻塞：300ms 防抖 + 版本合并，后台编译，不挡交互请求。
  (300 ms matches common LSP defaults, e.g. Apex LS's
  `documentChangeDebounceMs`.)

This is the standard pattern: keep the slow analysis off the interactive path
and coalesce bursts (cf. rust-analyzer#10075, Deno vscode#689).
这是通用模式：慢分析让出交互路径，连发合并（参见 rust-analyzer#10075、Deno vscode#689）。

---

# Development & testing · 开发与测试

The project is developed test-first (TDD). Run the suite / 本项目采用 TDD，运行测试套件：

```bash
cargo test                                   # Rust unit + integration · 单元+集成测试
guile -L deps -L tests/scheme \              # Scheme adapter tests · Scheme 适配测试
      -s tests/scheme/test-helpers.scm
bash tests/lsp_smoke.sh                      # stdio initialize smoke · 握手冒烟
```

---

# Acknowledgements · 致谢

- **Geiser** (Jose Antonio Ortega Ruiz) — the Guile analysis engine, vendored
  under `deps/mr-guile-lsp/geiser/` (Modified BSD License). · Guile 分析引擎
- **rgherdt/scheme-lsp-server** (Ricardo G. Herdt) — `lsp-helpers.scm` is ported
  from its Guile implementation (MIT). · 适配层移植来源

See [deps/README.md](deps/README.md) and [deps/MODIFICATIONS.md](deps/MODIFICATIONS.md)
for sources, licensing, and changes made to vendored code.
来源、许可与对内置代码的改动记录详见上述两文件。

---

# License · 许可

The project's own code is licensed under the **MIT** License (see `LICENSE`).
本项目自有代码采用 **MIT** 许可（见 `LICENSE`）。

**Per-file licensing · 逐文件许可：**

| Files · 文件 | License · 许可 | Copyright · 版权 |
|---|---|---|
| `src/*.rs`, `build.rs`, `tests/*.rs` | MIT | mr-guile-lsp-server contributors |
| `deps/mr-guile-lsp/lsp-helpers.scm` | MIT (port of MIT code) | contributors; portions © 2021 Ricardo G. Herdt (scheme-lsp-server, MIT) |
| `deps/mr-guile-lsp/geiser/*.scm` | **BSD-3-Clause** (vendored, unmodified logic) | © Jose Antonio Ortega Ruiz (Geiser) |

The two upstream dependencies we bundle (Geiser = BSD-3-Clause,
scheme-lsp-server = MIT) are both *permissive*, so MIT is compatible and no
copyleft is imposed; the BSD-3-Clause Geiser files keep their own license in
`deps/`. · 内置的两个上游依赖（Geiser=BSD-3、scheme-lsp-server=MIT）均为宽松许可，故 MIT 兼容且无 copyleft；BSD-3 的 Geiser 文件保留其原许可置于 `deps/`。

Full third-party sources, licenses, and the record of all adaptations are in
[deps/README.md](deps/README.md) and [deps/MODIFICATIONS.md](deps/MODIFICATIONS.md).
第三方来源、许可与全部适配改动记录见上述两文件。
