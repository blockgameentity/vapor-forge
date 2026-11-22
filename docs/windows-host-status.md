# Windows host — status

First live run: see [`windows-live-test.md`](./windows-live-test.md) for the install
layout, where the report bundle lands, and the assumption checklist.

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
| manifest providers owned by the host, not the backend | done — zero provider hostnames in this tree | `crates/config/src/model.rs`, `crates/features/src/request_code.rs` |
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
| loadable DLL: `DllMain` + worker thread + report bundle | done, compiled for `x86_64-pc-windows-gnu`, **never loaded by Steam** | `crates/host-windows/src/entry*`, `src/thread.rs` |
| hook bodies: ownership, package/marker pass, depot pre-seed consult, ticket decision | done; 3 of 4 have a corpus anchor, the ticket body has none | `crates/host-windows/src/hooks/*`, `src/abi/*` |
| behavioural probes (msabi + 4 profiles) | done, **all four targets still open** | `crates/patterns/src/{msabi,probe_facts,probe_rank}.rs`, `res/patterns/win64/PROBE-REPORT.md` |
| depotcache pre-seed logic | done (traits only, no host wiring) | `crates/features/src/depotcache.rs` |
| egress policy check | done | `scripts/check-egress.sh` |
| corpus drift CI | done | `.github/workflows/pattern-drift.yml` |
| proxy DLL build (`version.dll` forwarding to System32, loads the host DLL) | done, built and inspected; never loaded by Steam | `crates/host-windows/{build.rs,proxy/}`, `scripts/build-proxy.sh` |
| Windows CI job (cross-build + proxy export assertion + artifacts) | done | `.github/workflows/windows.yml` |
| config → feature flags bridge | done | `crates/features/src/config_apply.rs`, called from `ensure_runtime_services_for_config` |

## Verification (all run on Linux, no Windows box)

```sh
cargo test --workspace                                  # 1185 passed, 0 failed
cargo clippy --workspace --all-targets -- -D warnings    # clean
cargo check --workspace --all-targets                    # clean
cargo build -p vapor-forge-host-windows --target x86_64-pc-windows-gnu   # ok, emits the host DLL
objdump -p target/proxy/x86_64-pc-windows-gnu/version.dll                 # forwards to System32\version.dll.<name>
scripts/check-egress.sh                                  # ok
cargo test -p vapor-forge-host-windows                   # 104 passed
cargo clippy -p vapor-forge-host-windows --all-targets -- -D warnings   # clean
cargo build -p vapor-forge-host-windows --target x86_64-pc-windows-gnu  # DllMain exported
./target/debug/vapor-forge-pattern-mine --dll <public steamclient64.dll> \
  --module steamclient --arch x86_64 \
  --verify res/patterns/win64/steamclient/669fa08e….toml  # all 14 patterns resolve
```

`cargo build --target x86_64-pc-windows-gnu` for the **whole workspace** still fails,
and has done since before the host was loadable: `crates/diagnostics` calls
`libc::write(2, …, bytes.len())`, whose Windows signature takes a `u32` length. The
host DLL itself builds for that target. `i686-pc-windows-gnu` is not installed in
this environment, so the 32-bit build is untested here.

## What the hook bodies actually do

| hook | corpus symbol (public / publicbeta) | decision it applies |
| --- | --- | --- |
| ownership | `CUser::CheckAppOwnership` (yes / yes) | `features::apps::decide_check_ownership` + `features::family` provenance, and the borrower marker for a named lender |
| package | `CPackageInfo::GetPackageInfo` (yes / yes) | `features::package` marker housekeeping (`family::marker::remove_if_unborrowed`) over the controlled set |
| depot | `GetDecryptionKey` (yes / yes) | `features::depotcache`'s `satisfies_install` for the depot the key is for — reported, never fetched or written |
| ticket | `CUser::GetAppTicket` (**no / no**) | `features::ticket` source chain + authorisation window; the body cannot install, and would not serve bytes if it could |

Nothing is served from a ticket buffer and no package structure is rewritten: the
out-buffer layouts those would need are not measured on any Windows build.


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
- The DLL is loadable and exports `DllMain`, but **it has never been loaded into a
  Steam process**. Every assumption the owner has to check on a live client is
  listed at the top of `crates/host-windows/src/abi*` and in the delivery notes:
  `CAppOwnershipInfo`'s layout, whether `r8` of `CheckAppOwnership` is the out
  pointer, whether `GetPackageInfo`'s 32-bit argument is a package id, and which
  of the three `<host dir>/…` corpus layouts Steam installs actually use.
- The worker polls for `steamclient64.dll` / `SteamUI.dll` for ten seconds and then
  gives up; a module mapped later is missed for the process lifetime.
- `features::depotcache`'s pre-seed is consulted at `GetDecryptionKey`, because no
  request-code site (`CUser::BuildSpawnEnvBlock`) is anchored. Nothing skips
  Steam's request-code round trip yet.
- Windows corpus covers 14/26 (steamclient) and 7/10 (steamui) symbols on public.
  The remainder were never published as hints; behavioural probing did not
  identify them (`PROBE-REPORT.md`), and vtable anchoring found one candidate.
