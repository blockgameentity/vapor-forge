# Windows host — status

Branch: `integration/windows-host` (merges `phase1/hook-install` and
`gate0/behaviour-probes` on top of `phase3/depotcache-ci`). `main` untouched.

## What exists

| area | state | where |
| --- | --- | --- |
| `.pdata` — one definition, anchor body gate | done — canonical extents: 52,946 / 53,430 (steamclient public/beta), 40,826 / 41,159 (steamui) | `crates/patterns/src/pdata*`, `--strict` anchor check in the miner |
| `hook-engine` builds for Windows (VirtualProtect backend) | done | `crates/hook-engine/src/protect*` |
| Windows detour writer inside the engine (`writer::*`, prepare/enable/disable, restore on drop) | done; one planner, host re-exports it | `crates/hook-engine/src/writer*`, `crates/host-windows/src/install/patch.rs` |
| tickets: ordered sources, authorisation window, Denuvo detection + override | done, flag-gated | `crates/features/src/ticket/*` |
| family share: provenance, idempotent marker removal, concurrent-play decision | done, flag-gated | `crates/features/src/family/*` |
| host features parity (cloud, identity, depotcache, achievements flag) | done | `crates/host-windows/src/features/*`, `lifecycle` |
| UI split (render vs decide) documented | done | `[CT] docs/ui-split.md` |
| trampoline patch writer (instruction-boundary, relocatable-only) | done | `crates/host-windows/src/install/patch.rs` + `installer.rs` |
| provider failover opt-in, dependency-aware depot closure | done | `crates/features/src/{request_code,manifest}*`, `crates/config` |
| achievement rebuild from local definitions | done | `crates/features/src/achievements*` |
| vtable enumerator + slot evidence | done, **1 candidate only** | `crates/patterns/src/vtable_pe*`, `res/patterns/win64/VTABLE-REPORT.md` |
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
cargo test --workspace                                  # 1134 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings    # clean
cargo check --workspace --all-targets                    # clean
cargo build -p vapor-forge-host-windows --target x86_64-pc-windows-gnu   # ok
scripts/check-egress.sh                                  # ok
./target/debug/vapor-forge-pattern-mine --dll <public steamclient64.dll> \
  --module steamclient --arch x86_64 \
  --verify res/patterns/win64/steamclient/669fa08e….toml  # all 14 patterns resolve
```

## Known limits

- **Nothing has run inside a live Windows Steam client.** Every behavioural claim
  below the planner is fake-installer or synthetic-PE tested; cross-compilation
  proves types, not behaviour.
- `hook-engine` now owns one patch planner used by both hosts and a Windows
  writer with a prepare/enable/disable state machine; what is still unproven is
  any of it running in a real Windows Steam process.
- Of the four underived symbols, only `CUser::GetSubscribedApps` has a candidate
  (CUser vtable slot 183, **unconfirmed**). The other three have none
  (`res/patterns/win64/VTABLE-REPORT.md`).
- Phases 4 and 5 exist as gated semantics and host wiring; the Millennium UI
  migration itself is documented (`[CT] docs/ui-split.md`) but not performed, so
  the two UI stacks still coexist.
- Capabilities whose symbol is simply absent from a present corpus stay enabled;
  only unresolved/failed symbols disable dependents.
- No Windows runtime test: cross-compilation and fake-installer tests only.
- Windows corpus covers 14/26 (steamclient) and 7/10 (steamui) symbols on public.
  The remainder were never published as hints; behavioural probing did not
  identify them (`PROBE-REPORT.md`), and vtable anchoring found one candidate.
