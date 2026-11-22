# Parity: Linux host, Windows host, and the HYJ/BST feature set

Three things get called "parity" in this repo, so this file separates them:

1. **Host parity** — the same *capability set* on both hosts, so a report means
   the same thing on Linux and Windows.
2. **Behaviour parity** — a capability that is actually implemented (a body exists)
   *and* installable (the corpus anchors it) on that host.
3. **Feature parity with the Windows C++ fork** (`huanyuejue/OpenSteamTool` plus the
   additive ideas from `madoiscool/BetterSteamTools`) — the bar this project was
   started against.

Measured 2026-11-22 on `integration/windows-host`.

---

## 1. Host parity — done

Both hosts now name the same 21 capabilities (`crates/host-windows/src/capability.rs`
vs `crates/hooks/src/client/install/mod.rs`; Windows adds `Achievements` and
`RequestCodes`, which Linux models inside its own batches).

`cargo test -p vapor-forge-host-windows` asserts the enum, its labels and the
"disable one, leave the others" contract.

## 2. Behaviour parity — the real table

`body` = a replacement function exists on that host. `anchor` = the corpus has a
signature that resolves on the current public build.

| capability | Linux (module) | Windows body | Windows anchor |
| --- | --- | --- | --- |
| CallbackEvents | `client/{user,internal_callbacks,callback_notify,client_id,steam_context}.rs` | — | — |
| Ownership | `client/ownership.rs` | ✅ `CUser::CheckAppOwnership` | ✅ public + beta |
| PackageInjection | `client/package.rs`, `install/package_info.rs` | ✅ `CPackageInfo::GetPackageInfo` | ✅ |
| TicketOverrides | `client/{ticket,eticket,user}.rs` | ✅ `CUser::GetAppTicket` | ✅ public + beta (mined from the CUser vtable, 2026-11-22) |
| DepotInjection | `client/depot.rs` | ✅ `GetDecryptionKey` | ✅ |
| ShaderCacheControl | `client/shader.rs` | — | ✅ `CShaderCacheManager::…` (steamclient, unanchored in win64 corpus) |
| FamilyLockBypass | `client/family_lock.rs` | ⚠️ decision only, no body consults it | n/a |
| DlcOverrides | `client/dlc.rs` | — | ✅ `CPackageInfo::…` |
| CmInterception | `client/network.rs` | — | ❌ `CNetPacket` dispatch unanchored |
| NativeResponseDelivery | `client/network.rs` | — | — |
| CloudControl | `client/cloud.rs` | ⚠️ host feature pass decides, no client body | n/a |
| CloudHttp | `client/cloud_http.rs` | — | — |
| LaunchEnvironment | `client/env.rs` | — | `CUser::BuildSpawnEnvBlock` not anchored |
| LegacyCdKeyControl | `client/legacy_cdkey.rs` | — | — |
| SteamIdOverride | `client/user.rs` | — | — |
| LibraryUi | `ui/library.rs` | ✅ `ShouldShowAppInLibrary`, `FillInAppOverview`, `BuildCompleteAppOverviewChange`, `MarkAppChange` | ✅ (4 of 8 SteamUI symbols used) |
| OverviewMetadata | `ui/library.rs` | ✅ via the overview bodies | ✅ |
| LibrarySnapshot | `ui/library.rs` | ✅ via the snapshot body | ✅ |
| ConflictUiBridge | `ui/reverse_bridge.rs` | — | — |
| Achievements | `client/{achievement,user_stats}.rs` | ❌ body absent | ❌ stats path unanchored (class fully named, target shape absent) |
| RequestCodes | `client/network.rs` (via CmInterception) | ❌ nothing calls it | ❌ |

Counts: **Windows 8 of 21 capabilities have a body, 7 of them installable**;
Linux implements ~19. Ticket moved from "written but unanchored" to installable
when the CUser vtable pass mined `CUser::GetAppTicket` for both live builds.

Corpus coverage: Linux ships **34** symbols, Windows **23** (union of public +
publicbeta). Windows bodies exist for 8, of which 7 resolve.

## 3. Gap classes, and what each one costs

**A. No body (wiring).** The capability is named, the anchor exists, nothing
implements it: `ShaderCacheControl`, `DlcOverrides`, `CallbackEvents`,
`CloudHttp`, `LegacyCdKeyControl`, `SteamIdOverride`, `ConflictUiBridge`,
`LaunchEnvironment`. Linux has a working reference for each. This is mechanical
work — one body plus its decide function and tests per capability.

**B. No anchor (corpus).** Body or not, shape-based derivation found nothing for:
`CNetPacket` dispatch, the stats path, `CUser::GetSubscribedApps` (one unconfirmed
vtable candidate, slot 183) and `CUser::BuildSpawnEnvBlock`. These block
CmInterception, Achievements and the request-code skip on Windows. `GetAppTicket`
was the one that succeeded — slot-anchored in both builds — which is the evidence
that vtable passes produce anchors where prologue hunting does not. Vtable anchoring is
the only method that has produced *any* candidate so far.

**C. Unverified ABI.** Everything below the planner assumes MS-x64 argument
shapes taken from the Linux host. See `docs/windows-live-test.md` §4 for the list
and the observable that confirms or refutes each.

**D. Platform asymmetry, not a gap.** `audit-loader`/`proton-inject` are
Linux-only by construction; the proxy `version.dll` is the Windows injection
mechanism. Conversely the Windows fork's launch repairs (SteamStub, Error 54)
have no Linux analogue because Proton handles that path.

## 4. Against the HYJ/BST feature set

| feature | HYJ / BST | VF Linux | VF Windows |
| --- | --- | --- | --- |
| Lua ingest (`addappid`/`setmanifestid`) | ✅ | ✅ | ✅ loaded; visible once LibraryUi bodies install |
| fake ownership | ✅ | ✅ | ✅ body + anchor, ABI unverified |
| library UI / purchase time / removal | ✅ | ✅ | ✅ bodies (new) |
| family label + borrower marker | ✅ | ⚠️ decisions exist, hooks partial | ✅ in ownership/package bodies |
| concurrent play | ✅ | ✅ `family_lock.rs` | ⚠️ decision only |
| tickets | ✅ | ✅ | ❌ no anchor |
| Denuvo lease/pulse | ✅ | ⚠️ model + authorisation window | ❌ blocked by the ticket anchor |
| request codes / manifest download | ✅ 6 providers | ✅ CM interception | ❌ no anchor |
| depotcache pre-seed (BST) | — | ⚠️ logic exists, consulted on Windows only | ✅ |
| achievements (spoof, groups, local rebuild) | ✅ | ✅ | ❌ no stats anchor |
| launch robustness (SteamStub, 54, early inject) | ✅ many commits | n/a (Proton) | ⚠️ proxy exists, none of these repairs |
| cloud / identity / rich presence / toasts | partial | ✅ | ✅ host pass |
| providers (6, failover, CT-owned) | HYJ 6 silent, BST 3 | ✅ | ✅ |
| trust / egress control | HYJ GET-only | ✅ allowlist + CI | ✅ same code |

**Summary.** Linux is at or beyond HYJ parity everywhere except the Denuvo
lease behaviour (model exists, not wired into a body) and the BST additions that
were wired on the Windows host first (depotcache pre-seed consult, achievement
rebuild flag). Windows is at host parity, at partial behaviour parity: the ingest
path (Lua → ownership → library) is now wired end to end, while tickets, Denuvo,
request codes and achievements are blocked on corpus class B.

## 5. Order of work to close the remaining distance

1. **Concurrent-play consult** in the ownership body (Linux has `family_lock.rs`;
   the Windows decision exists and has no caller). Small.
2. **Statistics path anchor** (class B) — unblocks achievements *and* the local
   rebuild that BST contributed.
3. **`CUser::GetAppTicket` anchor** (class B) — unblocks tickets and Denuvo.
4. **`CNetPacket` dispatch anchor** (class B) — unblocks request codes, the last
   piece of the HYJ install path.
5. **Body-per-capability sweep** (class A) against the Linux reference:
   shader control, DLC overrides, callbacks, legacy CD key, SteamID override,
   cloud HTTP, conflict UI.
6. **Linux catch-ups** so the two hosts converge from both sides: wire
   `features::family` + `features::depotcache` into the Linux batches the way the
   Windows host does, and give Denuvo lease mode a caller.
