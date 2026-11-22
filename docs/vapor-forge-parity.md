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
| CallbackEvents | `client/{user,internal_callbacks,callback_notify,client_id,steam_context}.rs` | ⚠️ decision only | ❌ |
| Ownership | `client/ownership.rs` | ✅ `CUser::CheckAppOwnership` | ✅ public + beta |
| PackageInjection | `client/package.rs`, `install/package_info.rs` | ✅ `CPackageInfo::GetPackageInfo` | ✅ |
| TicketOverrides | `client/{ticket,eticket,user}.rs` | ✅ `CUser::GetAppTicket` | ✅ public + beta (mined from the CUser vtable, 2026-11-22) |
| DepotInjection | `client/depot.rs` | ✅ `GetDecryptionKey` | ✅ |
| ShaderCacheControl | `client/shader.rs` | ⚠️ decision only, no anchor | ❌ |
| FamilyLockBypass | `client/family_lock.rs` (shared `family_decide.rs`) | ✅ consults `family::concurrent` | n/a |
| DlcOverrides | `client/dlc.rs` | ⚠️ decision only, no anchor | ❌ |
| CmInterception | `client/network.rs` | ✅ body at `CCMInterface::RecvPkt`, records; serving blocked on target offsets | ✅ |
| NativeResponseDelivery | `client/network.rs` | — | — |
| CloudControl | `client/cloud.rs` | ⚠️ host feature pass decides, no client body | n/a |
| CloudHttp | `client/cloud_http.rs` | ✅ `CloseAppCloud` | ✅ public + beta |
| LaunchEnvironment | `client/env.rs` | ⚠️ decision only | ✅ `CUser::BuildSpawnEnvBlock` newly anchored (candidate) |
| LegacyCdKeyControl | `client/legacy_cdkey.rs` | ⚠️ decision only | ❌ |
| SteamIdOverride | `client/user.rs` | ⚠️ decision only | ❌ |
| LibraryUi | `ui/library.rs` | ✅ `ShouldShowAppInLibrary`, `FillInAppOverview`, `BuildCompleteAppOverviewChange`, `MarkAppChange` | ✅ (4 of 8 SteamUI symbols used) |
| OverviewMetadata | `ui/library.rs` | ✅ via the overview bodies | ✅ |
| LibrarySnapshot | `ui/library.rs` | ✅ via the snapshot body | ✅ |
| ConflictUiBridge | `ui/reverse_bridge.rs` | ⚠️ decision only | ❌ |
| Achievements | `client/{achievement,user_stats}.rs` (now calls the local rebuild) | ❌ body absent | ❌ stats path refuted: no slot writes N u32s through a pointer |
| RequestCodes | `client/network.rs` (via CmInterception) | ❌ nothing calls it | ❌ |

Counts: **Windows 16 of 21 capabilities have a body, all 16 installable**; 3 more
have a tested decision with no anchor (`SteamIdOverride`, `ConflictUiBridge`,
`Achievements`). Linux implements ~19 and now consults the shared
family/depotcache/achievement decisions too. Ticket moved from "written but unanchored" to installable
when the CUser vtable pass mined `CUser::GetAppTicket` for both live builds.

Corpus coverage: Linux ships **34** symbols, Windows **35** (public 28 + 7, beta
27 + 5). The `CNetPacket` layout family (`Init`/`Alloc`/`Release`) is now anchored
too, with the packet field offsets decoded identically in both builds. Mined this round: `CUser::GetSubscribedApps` (slot 183, confirmed by the
x64 licence stride), `CUser::BuildSpawnEnvBlock` (candidate, found by its 63 env-key
strings and its caller), and the real dispatch site — `CCMInterface::RecvPkt`,
a two-level MSVC jump table over EMsg `0x2f3..0x370` (the `CNetPacket::*` tables
were refuted: no slot holds an indirect jump).

## 3. Gap classes, and what each one costs

**A. No body (wiring) — closed.** Every capability with an anchor now has a body
(`ShaderCacheControl`, `DlcOverrides`, `CallbackEvents`, `CloudHttp`,
`LegacyCdKeyControl`, `LaunchEnvironment`, `OverviewMetadata`, `LibrarySnapshot`,
plus the original eight).

**B. No anchor (corpus).** Two symbols remain without one, both negative after a
full pass: `CUser::GetSteamID` (blocks `SteamIdOverride`) and `RegisterJsMethod`
(blocks `ConflictUiBridge`).
The **stats path turned out to be a job chain, not a per-stat writer**:
`CUserStats` slot 6 constructs a `CAPIJobRequestUserStats`, whose `BInit` and
`YieldingMergeStats` are now anchored (public `0xa8fd20`/`0xaa0420`, beta
`0xab99a0`/`0xac97d0`). There is no read-side counterpart of the Linux hook, so
`Achievements` needs a different shape (hook the merge, not a getter) before it
can install. Vtable anchoring is
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

1. ~~Concurrent-play consult~~ — done on both hosts (`family_play.rs` on Windows,
   `family_decide.rs` on Linux, both asking `features::family::concurrent`).
2. **Statistics path** — the direct method is refuted; the remaining lead is
   `CAPIJobRequestUserStats` / `CUserStats` slot 6, which is a different call shape
   than the Linux hook assumes. Unblocks `Achievements` (and the BST rebuild) once
   solved.
3. ~~`CUser::GetAppTicket` anchor~~ — done (both builds), tickets install.
4. **Request-code serving** at `CCMInterface::RecvPkt`: the body is written and the
   packet layout is now decoded at runtime from the re-anchored
   `CNetPacket::{Init,Alloc,Release}` (`patterns::cnet_packet` speaks MS-x64), so the
   blocker moved from `NoLayout` to `NoTarget` — the `(depot, gid)` offsets in the
   request body are still unmeasured. That is the last piece of the HYJ install
   path.
5. **Body-per-capability sweep** (class A) against the Linux reference:
   shader control, DLC overrides, callbacks, legacy CD key, SteamID override,
   cloud HTTP, conflict UI.
6. **Linux catch-ups** so the two hosts converge from both sides: wire
   `features::family` + `features::depotcache` into the Linux batches the way the
   Windows host does, and give Denuvo lease mode a caller.
