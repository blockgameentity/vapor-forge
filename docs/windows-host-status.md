# Windows host — status

Branch: `integration/windows-host` (merges `phase1/hook-install` and
`gate0/behaviour-probes` on top of `phase3/depotcache-ci`). `main` untouched.

## What exists

| area | state | where |
| --- | --- | --- |
| PE scanning (container-agnostic engine) | done | `crates/patterns/src/pe_image.rs`, `scan.rs` (`CodeSegment`, `scan_code_segment`) |
| `.pdata` function table, anchors, cross-build shape match | done | `crates/patterns/src/bin/vapor-forge-pe-functions.rs` |
| corpus miner (hints → unique wildcard sigs, `--verify`) | done | `crates/patterns/src/bin/vapor-forge-pattern-mine.rs` |
| Windows corpus (public + publicbeta) | done | `res/patterns/win64/<component>/<sha256>.toml` |
| module identity, capability gate, proxy plan, Win32 FFI | done | `crates/host-windows/src/{module,capability,proxy,inject}.rs` |
| install layer (corpus → BatchPlan → DetourInstaller) | done, no live client | `crates/host-windows/src/install*` |
| behavioural probes (msabi + 4 profiles) | done, **all four targets still open** | `crates/patterns/src/{msabi,probe_facts,probe_rank}.rs`, `res/patterns/win64/PROBE-REPORT.md` |
| depotcache pre-seed logic | done (traits only, no host wiring) | `crates/features/src/depotcache.rs` |
| egress policy check | done | `scripts/check-egress.sh` |
| corpus drift CI | done | `.github/workflows/pattern-drift.yml` |

## Verification (all run on Linux, no Windows box)

```sh
cargo test --workspace                                  # 946 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings    # clean
cargo check --workspace --all-targets                    # clean
cargo build -p vapor-forge-host-windows --target x86_64-pc-windows-gnu   # ok
scripts/check-egress.sh                                  # ok
./target/debug/vapor-forge-pattern-mine --dll <public steamclient64.dll> \
  --module steamclient --arch x86_64 \
  --verify res/patterns/win64/steamclient/669fa08e….toml  # all 14 patterns resolve
```

## Known limits

- The Win32 detour writer patches a 12-byte absolute jump with **no trampoline**,
  so a replacement cannot call the original yet, and it does not verify that the
  patch stays inside the target function.
- `hook-engine` does **not** build for Windows (`libc::mprotect`, `sysconf`,
  `PROT_*` unconditionally; `pic_thunk.rs` is 32-bit ELF repair). The
  `DetourInstaller` trait is the seam; the engine needs a page-protection
  abstraction and real trampolines.
- Capabilities whose symbol is simply absent from a present corpus stay enabled;
  only unresolved/failed symbols disable dependents.
- No Windows runtime test: cross-compilation and fake-installer tests only.
- Windows corpus covers 14/26 (steamclient) and 7/10 (steamui) symbols on public.
  The remainder were never published as hints; behavioural probing did not
  identify them (`PROBE-REPORT.md`).
