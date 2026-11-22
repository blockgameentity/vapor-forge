# Windows host — live test runbook

For the first run of the Windows host in a real Steam client. Nothing in this
document has been executed against Steam yet: the host compiles, the proxy
forwards, and every decision below the planner is unit-tested through fakes —
but no byte of it has run inside `steam.exe`.

Read this with `docs/windows-host-status.md` open; that file lists what exists,
this one lists what to do and what to distrust.

---

## 1. Build the two artifacts

From the repo root, on Linux or Windows with the mingw toolchain:

```sh
cargo build -p vapor-forge-host-windows --target x86_64-pc-windows-gnu
scripts/build-proxy.sh            # or: PROXY_OUT_DIR=<dir> scripts/build-proxy.sh
```

| artifact | path | notes |
| --- | --- | --- |
| host | `target/x86_64-pc-windows-gnu/debug/vapor_forge_host_windows.dll` | `DllMain` is the PE entry path (`DllMainCRTStartup` → `DllMain`) |
| proxy | `target/proxy/x86_64-pc-windows-gnu/version.dll` | 16 forwards into `C:/Windows/System32/version.dll` |

`build.rs` runs the proxy script automatically for the Windows target **only when
the mingw compiler is present**; otherwise it prints a `cargo:warning` and the
build still succeeds. If the warning appears, run the script by hand.

Release build: append `--release` and use `target/x86_64-pc-windows-gnu/release/`.

---

## 2. Install layout

Next to `steam.exe` (the folder that holds `steamclient64.dll`):

```
<Steam>/
  steam.exe
  version.dll                        <- the proxy (this is the injection point)
  vapor_forge_host_windows.dll       <- the host the proxy loads
  patterns/win64/<component>/<sha>.toml     (or res/patterns/..., see §5)
  config.toml is read from %USERPROFILE%\.config\vapor-forge\config.toml
```

The corpus the host looks for, in order — first directory that contains a
`win64/` subdirectory wins:

1. `<host dll dir>/patterns`
2. `<host dll dir>/res/patterns`
3. `<host dll dir>/../res/patterns`
4. `<config dir>/patterns`

Copy `res/patterns/win64/<component>/<sha256>.toml` from this repo, keyed by the
**sha256 of the DLLs on that machine** — a corpus file for a different build will
not resolve. Get the sha with
`certutil -hashfile steamclient64.dll SHA256` and compare against the filenames
under `res/patterns/win64/steamclient/`.

---

## 3. First launch, and where the answers are

Launch Steam normally. The proxy loads the host, which starts one worker thread.

Two files matter:

| file | contents |
| --- | --- |
| `%USERPROFILE%\.config\vapor-forge\logs\windows-host.log` | every `tracing` event, including per-symbol resolution and every capability that disabled itself |
| `%USERPROFILE%\.config\vapor-forge\reports\windows-host.txt` | the report bundle: environment notes, per-module install report, feature report, and the gate report |

Both are best-effort: if they cannot be written the host still runs, and the log
says where writing failed.

**The report bundle is the deliverable of this test.** It names each installed
symbol with its resolved address, and each disabled capability **with the reason**
it was disabled — which is the fastest way to tell an unanchored symbol from a
wrong ABI.

---

## 4. Assumptions to confirm or refute

Each of these is currently a *guess from the Linux host*, not a measurement on a
Windows build. Written so each one has an observable outcome.

| # | assumption | how to confirm | if it is wrong |
| --- | --- | --- | --- |
| 1 | `CAppOwnershipInfo` layout (`#[repr(C, packed)]`, 0x38 bytes) matches `steamclientpublic.h` | steam owns an app and the library shows it; no crash on ownership query | garbage fields, possible crash on first ownership check → disable `ownership` (see §6) |
| 2 | `r8` of `CheckAppOwnership` is the `CAppOwnershipInfo*` out pointer | ownership decisions show up in the library at all | Steam sees `false` everywhere → apps disappear from library; nothing is written to the wrong place if the call falls through |
| 3 | `GetPackageInfo`'s 32-bit argument is a package id | packages resolve; DLC list is right | wrong package counted, no crash |
| 4 | `<host dir>/patterns` (or one of the four candidates) is where the corpus ends up | the report names a corpus path and resolves >0 symbols | report shows "corpus not found" → move the corpus to a candidate path from §2 |
| 5 | the worker sees the Steam modules within 10 s of attach | report lists both `steamclient64.dll` and `SteamUI.dll` | a module mapped later is missed for the process lifetime → restart Steam with the proxy already in place |
| 6 | argv/return shapes for depot (`GetDecryptionKey`) and ticket (`GetAppTicket`) | a controlled app installs; ticket path reports its reason | see the log line naming the unpreventable capability |

The verified half of the ownership ABI is the prologue itself
(`crates/host-windows/src/abi/ownership.rs`): `rcx` 64-bit pointer, `rdx` 32-bit
id, anchored in both live builds. Everything else in that file is marked
*Assumed* — treat those comments as the checklist.

---

## 5. What is expected to be missing on the first run

These are known, not bugs to chase:

- **`ticket-overrides` installs** as of the vtable mining pass
  (`CUser::GetAppTicket`, both live builds). If it still disables itself, the
  report's reason is the evidence to send back.
- **`achievements` rewrites the reply** when `[achievements] local_rebuild` is on:
  the merge body decodes the response, substitutes the rebuilt set and restores
  Steam's bytes afterwards. If the flag is off, behaviour is exactly Steam's.
- **Request codes are fetched, then served.** The dispatch body parses the
  request payload, and if a code for that `(depot, gid)` is already cached it hands
  Steam the encoded response for that call; on a miss the hook only *enqueues* the
  `(app, depot, gid)` and a worker runs the provider chain
  (`features::request_code::PendingQueue`), so a later request for the same depot is
  served. Nothing dials on Steam's networking thread, and with no provider
  configured the worker stays idle and Steam's own round trip happens.
- **Corpus coverage:** 30 symbols in `steamclient64.dll` public (29 beta) and 9 in
  `SteamUI.dll` (7 beta), including the `CNetPacket` layout family and the leaf
  `CUser::GetSteamID` accessor. Unresolved symbols disable only their own
  capability.
- `CUser::GetSubscribedApps` is anchored at CUser slot 183 (confirmed by the x64
  licence stride) but has **no body**, so it stays uninstalled.

---

## 6. If it goes wrong

1. **Read the gate report first** — it names every disabled capability and why.
   That distinguishes "no anchor" (expected) from "wrong ABI" (needs a fix).
2. **Disable one capability at a time** in `config.toml` rather than removing the
   host. The flags that matter for this test:
   - `[apps.shared] validate_lender` / `clean_borrower_markers` — set `false` to
     restore pre-fix family labelling.
   - `[achievements] local_rebuild` — leave `false` unless testing that path.
   - `[ticket] denuvo_mode`, `source_order`, `denuvo_detection` — Denuvo paths.
3. **Remove the host entirely** by deleting `version.dll` next to `steam.exe`.
   Steam then loads the real system `version.dll` again. Nothing else is touched:
   the host writes no registry keys, no services, no scheduled tasks.
4. **Crash on launch before any log appears** means the failure is inside
   `DllMain` (loader lock) or the proxy itself. Remove `version.dll`; the log will
   be empty, which is itself the evidence.

---

## 7. What to send back

Attach the report bundle and the log, plus:

```text
Steam build (from the log), steamclient64.dll sha256, SteamUI.dll sha256,
Windows version, whether the proxy loaded at all, and the first symptom.
```

`certutil -hashfile <dll> SHA256` gives the two hashes; the report records the
build fingerprint it resolved.
