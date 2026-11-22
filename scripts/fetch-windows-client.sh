#!/usr/bin/env bash
# Fetch a Windows Steam client package and extract the modules the pattern
# corpus is keyed on.
#
#   scripts/fetch-windows-client.sh <public|publicbeta> <out-dir>
#
# Writes <out-dir>/steamclient64.dll and <out-dir>/SteamUI.dll plus
# <out-dir>/fetch.json (build id, zip sha256, module sha256). Read-only with
# respect to Steam: these are the same public CDN packages the client installs.
set -euo pipefail

channel="${1:?usage: fetch-windows-client.sh <public|publicbeta> <out-dir>}"
out="${2:?usage: fetch-windows-client.sh <public|publicbeta> <out-dir>}"
base="https://client-update.akamai.steamstatic.com"

case "$channel" in
  public)           manifest="$base/steam_client_win64" ;;
  publicbeta|beta)  manifest="$base/steam_client_publicbeta_win64" ;;
  *) echo "unsupported channel: $channel" >&2; exit 2 ;;
esac

mkdir -p "$out"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsS --max-time 60 "$manifest" -o "$tmp/manifest.vdf"

read -r build file < <(python3 - "$tmp/manifest.vdf" <<'PY'
import re, sys
text = open(sys.argv[1], "r", encoding="utf-8", errors="replace").read()
build = re.search(r'"version"\s+"(\d+)"', text).group(1)
section = re.search(r'"bins_win64"\s*\{(.*?)\n\t\}', text, re.S).group(1)
name = re.search(r'"file"\s+"(bins_win64\.zip\.[0-9a-f]+)"', section).group(1)
print(build, name)
PY
)

echo "channel=$channel build=$build package=$file"
curl -fsS --max-time 600 "$base/$file" -o "$tmp/bins.zip"
unzip -o -j -q "$tmp/bins.zip" 'steamclient64.dll' 'SteamUI.dll' -d "$out"

python3 - "$out" "$build" "$file" <<'PY'
import hashlib, json, pathlib, sys
out = pathlib.Path(sys.argv[1])
modules = {}
for name in ("steamclient64.dll", "SteamUI.dll"):
    data = (out / name).read_bytes()
    modules[name] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
(out / "fetch.json").write_text(json.dumps({"build": sys.argv[2], "package": sys.argv[3], "modules": modules}, indent=2) + "\n")
print(json.dumps(modules, indent=2))
PY
