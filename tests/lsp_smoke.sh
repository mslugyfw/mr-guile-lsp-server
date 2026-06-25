#!/usr/bin/env bash
# Minimal LSP smoke test: confirm the binary boots and answers initialize with
# the expected capabilities (utf-8 encoding, declared providers, server info).
# Per-feature behavior is covered by `cargo test --test integration`.
set -u
BIN="${1:-./target/debug/mr-guile-lsp-server}"
REQ='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"processId":1,"rootUri":null,"clientInfo":{"name":"smoke","version":"0"}}}'
RESP=$(printf 'Content-Length: %d\r\n\r\n%s' "${#REQ}" "$REQ" | timeout 3 "$BIN" 2>/dev/null)

ok=1
echo "$RESP" | grep -q '"name":"mr-guile-lsp-server"' || { echo "FAIL: server name"; ok=0; }
echo "$RESP" | grep -q '"positionEncoding":"utf-8"' || { echo "FAIL: utf-8 encoding"; ok=0; }
echo "$RESP" | grep -q 'completionProvider' || { echo "FAIL: completion provider"; ok=0; }
echo "$RESP" | grep -q 'definitionProvider' || { echo "FAIL: definition provider"; ok=0; }
echo "$RESP" | grep -q 'hoverProvider' || { echo "FAIL: hover provider"; ok=0; }

if [ "$ok" = 1 ]; then echo "SMOKE OK: initialize responds with all capabilities"; exit 0; else exit 1; fi
