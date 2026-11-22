#!/usr/bin/env bash
# Build the Windows proxy DLL that Steam loads in place of a system module.
#
#   scripts/build-proxy.sh [TARGET-TRIPLE]        (default x86_64-pc-windows-gnu)
#
# The artifact is `version.dll`: every export is a PE forwarder into
# C:/Windows/System32/version.dll (see crates/host-windows/proxy/version.def),
# and the only code it runs is DllMain mapping `vapor_forge_host_windows.dll`
# (see crates/host-windows/proxy/loader.c).
#
# Output: ${PROXY_OUT_DIR:-$CARGO_TARGET_DIR/proxy/<triple>}/version.dll, falling
# back to <repo>/target/proxy/<triple>/version.dll. The script prints that path
# on success. It is idempotent: same inputs, same bytes, and it removes a stale
# artifact before compiling so a failed build never leaves one behind that a
# later step could mistake for success.
#
# Needs a MinGW-w64 toolchain (cross gcc + objdump). Missing tools are a hard
# failure with the install command, never a skipped step.
set -euo pipefail

triple="${1:-x86_64-pc-windows-gnu}"
case "$triple" in
  x86_64-pc-windows-gnu) prefix=x86_64-w64-mingw32 ;;
  i686-pc-windows-gnu) prefix=i686-w64-mingw32 ;;
  *)
    echo "build-proxy: error: unsupported target '$triple' (want x86_64-pc-windows-gnu or i686-pc-windows-gnu)" >&2
    exit 2
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src_dir="$repo_root/crates/host-windows/proxy"
def_file="$src_dir/version.def"
loader_src="$src_dir/loader.c"

cc="${PROXY_CC:-${prefix}-gcc}"
if ! command -v "$cc" >/dev/null 2>&1; then
  echo "build-proxy: error: '$cc' not found; no proxy DLL was produced." >&2
  echo "build-proxy: install MinGW-w64 (Debian/Ubuntu: sudo apt-get install -y mingw-w64;" >&2
  echo "build-proxy: Fedora: sudo dnf install mingw64-gcc) or set PROXY_CC." >&2
  exit 1
fi
# A cross objdump is the reliable way to read PE exports; the host objdump works
# too when it was built with the PE BFD backend, so accept either.
if command -v "${prefix}-objdump" >/dev/null 2>&1; then
  objdump_bin="${prefix}-objdump"
elif command -v objdump >/dev/null 2>&1; then
  objdump_bin=objdump
else
  echo "build-proxy: error: neither '${prefix}-objdump' nor 'objdump' found; cannot verify exports." >&2
  exit 1
fi

# -shared           : a DLL, not an executable stub.
# -O2 -Wall -Wextra : loader.c is ~45 lines; a warning there is never noise.
# -nostdlib -lkernel32 : no C runtime. The loader calls three kernel32 entry
#                   points and nothing else, and CRT initialisation would run
#                   inside the loader lock - the one place a proxy must not do
#                   work it cannot prove safe. It also keeps the DLL free of the
#                   UCRT (api-ms-win-crt-*), so it loads on any Windows that can
#                   run Steam, with no redistributable to check.
# -fno-stack-protector : with no CRT there is no __stack_chk_fail to call.
# -Wl,--entry,DllMain : no CRT means no DllMainCRTStartup wrapper, so the entry
#                   point is DllMain itself (ld resolves it decorated on i686).
# -Wl,--no-insert-timestamp : byte-identical rebuilds, so a rebuild only differs
#                   when an input actually changed.
flags=(-shared -O2 -Wall -Wextra -nostdlib -fno-stack-protector -Wl,--entry,DllMain -Wl,--no-insert-timestamp)
# i686 only: MinGW decorates stdcall symbols as `_Name@N`, and a .def forwarder
# that keeps the decoration would export a name no caller imports. --kill-at
# strips it so the export table matches the documented undecorated names.
# x86_64 has a single calling convention and needs nothing.
case "$triple" in
  i686-*) flags+=(-Wl,--kill-at) ;;
esac

# Output directory, in order of precedence: PROXY_OUT_DIR (what build.rs passes),
# CARGO_TARGET_DIR (so a relocated target dir keeps the artifact next to the
# rest of the build), else the repository's own target dir.
if [ -n "${PROXY_OUT_DIR:-}" ]; then
  out_dir="$PROXY_OUT_DIR"
elif [ -n "${CARGO_TARGET_DIR:-}" ]; then
  case "$CARGO_TARGET_DIR" in
    /*) out_dir="$CARGO_TARGET_DIR/proxy/$triple" ;;
    *) out_dir="$repo_root/$CARGO_TARGET_DIR/proxy/$triple" ;;
  esac
else
  out_dir="$repo_root/target/proxy/$triple"
fi
mkdir -p "$out_dir"
artifact="$out_dir/version.dll"
rm -f "$artifact"

"$cc" "${flags[@]}" -o "$artifact" "$loader_src" "$def_file" -lkernel32

# Assert the artifact instead of trusting the linker: the export surface is the
# one contract a Steam update can break without any Rust code changing.
expected_names="$(sed -n -E 's/^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=.*/\1/p' "$def_file" | sort)"
expected_count="$(printf '%s\n' "$expected_names" | sed '/^$/d' | wc -l | tr -d ' ')"
actual_names="$(dump="$("$objdump_bin" -p "$artifact")";
  printf '%s\n' "$dump" \
    | sed -n -E 's/.*Forwarder RVA -- .*\.([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*$/\1/p' \
    | sort)"
actual_count="$(printf '%s\n' "$actual_names" | sed '/^$/d' | wc -l | tr -d ' ')"

if [ "$expected_count" != "$actual_count" ]; then
  echo "build-proxy: error: $def_file lists $expected_count exports but $artifact has $actual_count forwarders" >&2
  echo "build-proxy: expected: $(printf '%s ' $expected_names)" >&2
  echo "build-proxy: actual:   $(printf '%s ' $actual_names)" >&2
  exit 1
fi
if [ "$expected_names" != "$actual_names" ]; then
  echo "build-proxy: error: $artifact forwards a different set of names than $def_file" >&2
  diff <(printf '%s\n' "$expected_names") <(printf '%s\n' "$actual_names") >&2 || true
  exit 1
fi

echo "build-proxy: artifact $artifact ($(stat -c '%s' "$artifact") bytes, $actual_count forwarded exports)"
