#!/usr/bin/env bash
# Builds wasm-experiment/repl-core: veripb-repl without the TUI, for checking
# the core against wasm32-unknown-unknown.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(dirname "$here")"
out="$here/repl-core"

rm -rf "$out"
mkdir -p "$out/.cargo"
cp -r "$repo/src" "$repo/Cargo.toml" "$out/"
rm -rf "$out/src/tui" "$out/src/main.rs"
sed -i '/^pub mod tui;/d' "$out/src/lib.rs"
cp "$here/veripb-wasm/.cargo/config.toml" "$out/.cargo/"

python3 - "$out/Cargo.toml" <<'EOF'
import re, sys
p = sys.argv[1]
s = open(p).read()
s = s.replace('crossterm = "0.29"\n', '')
s = re.sub(r'\n\[\[bin\]\][^\[]*', '\n', s)
s = re.sub(r'\n\[dev-dependencies\][^\[]*', '\n', s)
s += ('\n[target.\'cfg(all(target_arch = "wasm32", target_os = "unknown"))\'.dependencies]\n'
      'getrandom = { version = "0.3", features = ["wasm_js"] }\n\n[workspace]\n')
open(p, 'w').write(s)
EOF

echo "created $out"
