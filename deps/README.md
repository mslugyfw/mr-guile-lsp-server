# deps/ — Vendored Scheme dependencies · 内置 Scheme 依赖

This folder vendors all Scheme code the Guile REPL subprocess needs at runtime,
so the project has **zero external Guile package install requirements**.
本目录内置了 Guile REPL 子进程运行时所需的全部 Scheme 代码，使项目**无需安装任何外部 Guile 包**。

## Layout · 目录结构

```
deps/
├── README.md            # this file · 本文件
├── MODIFICATIONS.md     # ★ every change to vendored third-party code · 对内置第三方代码的每一处改动记录
└── mr-guile-lsp/        # on Guile load-path (use: guile -L deps) · Guile 加载路径根
    ├── geiser/          # vendored Geiser guile modules (third-party) · 内置 Geiser 模块
    │   ├── completion.scm
    │   ├── doc.scm
    │   ├── evaluation.scm
    │   ├── modules.scm
    │   ├── utils.scm
    │   ├── xref.scm
    │   ├── emacs.scm
    │   └── LICENSE       # Geiser Modified BSD License · Geiser 许可
    └── lsp-helpers.scm  # OUR code (thin adapter, ported) · 自有代码（薄适配层，移植而来）
```

## What is vendored · 内置内容

### Geiser guile modules — `mr-guile-lsp/geiser/*.scm`

- **Source · 来源**: `codeberg.org/rgherdt/scheme-lsp-server` → `geiser/guile/src/geiser/`
  (originally from the [Geiser](https://geiser.nongnu.org/) project by
  Jose Antonio Ortega Ruiz / 原作者 Jose Antonio Ortega Ruiz)
- **License · 许可**: BSD-3-Clause (a.k.a. "Modified BSD License"; see
  `mr-guile-lsp/geiser/LICENSE`, preserved verbatim in each `.scm` header /
  即"Modified BSD"，每个文件头保留原文)
- **Purpose · 用途**: mature Guile semantic analysis — completions, signatures,
  symbol location (goto), module resolution, file load/compile. / 成熟的 Guile 语义分析：补全、签名、符号定位（跳转）、模块解析、文件加载/编译。
- **Dependencies · 依赖**: only Guile 3.0 core built-in modules
  (`ice-9 session`/`regex`/`documentation`, `system vm program`/`debug`,
  `system xref`, `system base compile`, `language tree-il`, `oop goops`,
  `texinfo`, `srfi srfi-1`). **No external libraries.** / 仅依赖 Guile 3.0 核心内置模块，**无外部库**。

### Our adapter — `mr-guile-lsp/lsp-helpers.scm`

- **Origin · 来源**: ported from scheme-lsp-server's
  `lsp-server/private/compat-guile-impl.scm` + `adapter-impl.scm`, with the
  JSON-RPC/LSP layer stripped (that now lives in Rust). / 移植自 scheme-lsp-server，去掉了 JSON-RPC/LSP 层（现由 Rust 处理）。
- **License · 许可**: project MIT; provenance noted in-file. / 项目 MIT，文件内标注来源。

## How to obtain / refresh · 如何获取/刷新

```bash
BASE=https://codeberg.org/rgherdt/scheme-lsp-server/raw/branch/master/geiser/guile/src/geiser
mkdir -p deps/mr-guile-lsp/geiser
for f in completion doc evaluation modules utils xref emacs; do
  curl -fsSL "$BASE/$f.scm" -o deps/mr-guile-lsp/geiser/$f.scm
done
curl -fsSL "https://codeberg.org/rgherdt/scheme-lsp-server/raw/branch/master/geiser/guile/license" \
  -o deps/mr-guile-lsp/geiser/LICENSE
# Apply the namespace rename (see MODIFICATIONS.md change #1) · 应用命名空间重命名（见 MODIFICATIONS.md 第 1 条）
sed -i 's/lsp-server geiser/mr-guile-lsp geiser/g' deps/mr-guile-lsp/geiser/*.scm
```

Any further adaptations must be recorded in **`MODIFICATIONS.md`**.
任何后续适配都必须记录在 **`MODIFICATIONS.md`** 中。

## Verification · 验证

The vendored modules should load in a clean Guile / 内置模块应能在干净 Guile 中加载：

```bash
guile -L deps -c '
(use-modules (mr-guile-lsp geiser completion)
             (mr-guile-lsp geiser doc)
             (mr-guile-lsp geiser modules)
             (mr-guile-lsp geiser xref)
             (mr-guile-lsp geiser evaluation))
(display "all geiser modules loaded / 全部 Geiser 模块已加载")(newline)'
```
