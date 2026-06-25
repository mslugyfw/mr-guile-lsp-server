# Modifications to vendored third-party code · 对内置第三方代码的改动记录

Every change made to third-party Scheme code (Geiser modules, or anything
ported from `rgherdt/scheme-lsp-server`) is recorded here so the provenance and
the diff against upstream stay auditable.

本文档记录对第三方 Scheme 代码（Geiser 模块，或任何移植自
`rgherdt/scheme-lsp-server` 的代码）所做的每一处改动，以保证来源可追溯、与上游的差异可审计。

Convention per change · 每条记录的约定格式：
- **#N** — short title · 简短标题
- **File(s)** · 涉及文件
- **Upstream (before)** → **This project (after)** · 上游（改前）→ 本项目（改后）
- **Reason** · 原因

---

## #1 — Namespace rename: `(lsp-server geiser …)` → `(mr-guile-lsp geiser …)`

- **Files**: all of `mr-guile-lsp/geiser/*.scm`
  (completion, doc, evaluation, modules, utils, xref, emacs)
- **Before** (upstream scheme-lsp-server):
  ```scheme
  (define-module (lsp-server geiser completion) …)
  #:use-module (lsp-server geiser utils)
  ```
- **After** (this project):
  ```scheme
  (define-module (mr-guile-lsp geiser completion) …)
  #:use-module (mr-guile-lsp geiser utils)
  ```
- **Reason**: avoid polluting the generic `(lsp-server …)` module namespace
  (which collides with the upstream package this was vendored from). Using our
  project's own namespace keeps the bundled Geiser self-contained and avoids
  confusing an installed `guile-lsp-server` with our server.
- **Applied via**:
  ```bash
  sed -i 's/lsp-server geiser/mr-guile-lsp geiser/g' deps/mr-guile-lsp/geiser/*.scm
  ```
- **Behavior impact**: none — purely a module-name relabeling. No logic changed.

---

## #2 — `lsp-helpers.scm`: ported + adapted from scheme-lsp-server

- **File**: `mr-guile-lsp/lsp-helpers.scm` (this project's own file, NOT a
  verbatim vendored file)
- **Origin**: ported from `rgherdt/scheme-lsp-server`:
  - `lsp-server/private/compat-guile-impl.scm` — the Guile-specific `$*`
    functions (`$apropos-list`, `$fetch-documentation`, `$fetch-signature`,
    `$get-definition-locations`, `$compute-diagnostics`, `$open-file!`)
  - `lsp-server/private/adapter-impl.scm` — the `lsp-geiser-*` wrappers
- **What was reused (same logic, same Geiser calls)**:
  - `lsp-completions` → `completions` (Geiser apropos) — like upstream
    `$apropos-list` / `lsp-geiser-completions`
  - `lsp-documentation` → `symbol-documentation` docstring extraction — like
    `$fetch-documentation` / `lsp-geiser-documentation`
  - `lsp-signature` → `autodoc` arg flattening — like `$fetch-signature` /
    `lsp-geiser-signature`
  - `lsp-find-definition` → `symbol-location` — like `$get-definition-locations`
    / `lsp-geiser-symbol-location`
- **What was removed**: the JSON-RPC / LSP framing layer (upstream
  `lsp-server-impl.scm`'s `json-rpc-loop`, handler table, capability
  negotiation) — this now lives in the **Rust** side (`tower-lsp`).
- **What was added (new, not in upstream)**:
  - `lsp-serve` / `lsp-eval` — a sentinel-delimited (`%%LSP-DONE%%`)
    request/response loop so the Rust parent can call `lsp-*` functions over a
    pipe. Upstream embeds the whole server in one Guile process instead.
  - `lsp-check-syntax` — `compile-file` with `current-warning-port` redirected
    and `catch` around compile errors; returns `((warnings . <text>)
    (error . <msg>|#f))`. Upstream `$compute-diagnostics` does similar but
    parses inside Scheme; we return raw text and let Rust parse
    (`src/diagnostics.rs`).
  - `lsp-load-file` — `ge:load-file` wrapped in `catch`, to load a buffer into
    the REPL for symbol introspection.
- **Module namespace**: `(mr-guile-lsp lsp-helpers)` (project's own, no clash).
- **Output convention**: every `lsp-*` query returns `#f` or a simple,
  `write`-friendly S-expression (see file header) so Rust can parse it with the
  project's own S-expr parser (`src/parser.rs`).
- **License**: project MIT (upstream scheme-lsp-server is MIT, so the
  port is compatible); provenance noted in the file header.

---

<!-- Append further changes below as #3, #4, … -->
