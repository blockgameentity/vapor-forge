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
| FamilyLockBypass | `client/family_lock.rs` (shared `family_decide.rs`) | ✅ body at `CUser::GetFamilySharedLockAccount` (the Windows family shared-lock query, not Linux's `CFamilySharing`) | ✅ public + beta |
| DlcOverrides | `client/dlc.rs` | ⚠️ decision only, no anchor | ❌ |
| CmInterception | `client/network.rs` | ✅ body at `CCMInterface::RecvPkt`, **serves** a cached code with a swap guard, and a worker **fetches** misses through `features::request_code::PendingQueue` | ✅ |
| NativeResponseDelivery | `client/network.rs` (queue + work item) | **retired on Windows**: serving is synchronous inside Steam's `RecvPkt` call, so there is no cross-thread hand-off to gate — mapped onto the serving symbol | n/a |
| CloudControl | `client/cloud.rs` | ⚠️ host feature pass decides, no client body | n/a |
| CloudHttp | `client/cloud_http.rs` | ✅ `CloseAppCloud` | ✅ public + beta |
| LaunchEnvironment | `client/env.rs` | ⚠️ decision only | ✅ `CUser::BuildSpawnEnvBlock` newly anchored (candidate) |
| LegacyCdKeyControl | `client/legacy_cdkey.rs` | ⚠️ decision only | ❌ |
| SteamIdOverride | `client/user.rs` | ✅ out-parameter body at `CUser::GetSteamID` (`0xcfec0`/`0xd0460`, leaf) | ✅ |
| LibraryUi | `ui/library.rs` | ✅ `ShouldShowAppInLibrary`, `FillInAppOverview`, `BuildCompleteAppOverviewChange`, `MarkAppChange` | ✅ (4 of 8 SteamUI symbols used) |
| OverviewMetadata | `ui/library.rs` | ✅ via the overview bodies | ✅ |
| LibrarySnapshot | `ui/library.rs` | ✅ via the snapshot body | ✅ |
| ConflictUiBridge | `ui/reverse_bridge.rs` | ✅ `RegisterJsMethod` body records bridge registrations | ✅ |
| Achievements | `client/{achievement,user_stats}.rs` (now calls the local rebuild) | ❌ body absent | ❌ stats path refuted: no slot writes N u32s through a pointer |
| RequestCodes | `client/network.rs` (via CmInterception) | ❌ nothing calls it | ❌ |

Counts: **Windows 21 of 21 capabilities accounted for**: 20 have a body and install and
`NativeResponseDelivery` is retired with evidence (synchronous serving). Linux implements ~19 and now consults the shared
family/depotcache/achievement decisions too. Ticket moved from "written but unanchored" to installable
when the CUser vtable pass mined `CUser::GetAppTicket` for both live builds, and
`FamilyLockBypass` stopped being the capability without a carrier: the eighth pass
of `res/patterns/win64/VTABLE-REPORT.md` found the family lock is decided
client-side in `CUser` and anchored the query both of Steam's own lock consumers
call (`0xa07460` public / `0xa2f7f0` publicbeta).

Corpus coverage: Linux ships **34** symbols, Windows **38** (public 30 + 8, beta
29 + 6). The `CNetPacket` layout family (`Init`/`Alloc`/`Release`) is now anchored
too, with the packet field offsets decoded identically in both builds. Mined this round: `CUser::GetSubscribedApps` (slot 183, confirmed by the
x64 licence stride), `CUser::BuildSpawnEnvBlock` (candidate, found by its 63 env-key
strings and its caller), `CUser::GetFamilySharedLockAccount` (the family
shared-lock query, eighth pass of `VTABLE-REPORT.md`), and the real dispatch site
— `CCMInterface::RecvPkt`, a two-level MSVC jump table over EMsg `0x2f3..0x370`
(the `CNetPacket::*` tables were refuted: no slot holds an indirect jump).

## 3. Gap classes, and what each one costs

**A. No body (wiring) — closed.** Every capability with an anchor now has a body
(`ShaderCacheControl`, `DlcOverrides`, `CallbackEvents`, `CloudHttp`,
`LegacyCdKeyControl`, `LaunchEnvironment`, `OverviewMetadata`, `LibrarySnapshot`,
plus the original eight).

**B. No anchor (corpus) — closed.** `CUser::GetSteamID` is anchored at its real
Windows shape (an out-parameter accessor, a proven leaf outside `.pdata`) and
`RegisterJsMethod` was already anchored; both now have bodies. The gate itself was
relaxed for proven leaves, which is what made the first possible.
The **stats path turned out to be a job chain, not a per-stat writer**:
`CUserStats` slot 6 constructs a `CAPIJobRequestUserStats`, whose `BInit` and
`YieldingMergeStats` are now anchored (public `0xa8fd20`/`0xaa0420`, beta
`0xab99a0`/`0xac97d0`). There is no read-side counterpart of the Linux hook, so
`Achievements` needs a different shape (hook the merge, not a getter) before it
can install. Vtable anchoring is
the only method that has produced *any* candidate so far.

**C. The achievements rebuild has no independent source yet — and cheesetool is not
it.** CT owns manifests, depot keys, Lua, Hubcap and LuaTools auth; it holds no
achievement schema and fetches none (checked: no `GetSchemaForGame`, no schema
store, nothing under `src/` or `cheesetool-sourcer`). Making it the source would
mean either handing it the same `appcache/stats/UserGameStatsSchema_*.bin` files VF
already parses — circular — or pointing it at a schema service / a WebAPI key,
which is precisely the third-party-egress category this project excludes. So the
flag stays off and nothing is fabricated. The one in-policy path: if CT's sourcer
ever archives schemas the way it archives manifests, CT would hold an *independent*
copy and writing definitions becomes both possible and honest.
 On *both* hosts the
"local" definition set is derived from the same schema cache the rebuild stands in
for (`hooks::achievement_local::definitions` and the Windows `reply::definitions`
both parse the cached schema). So `[achievements] local_rebuild` is off by default
and turning it on re-derives what it was given; a real parity feature needs an
independent definition source (for instance cheesetool writing achievement
definitions it owns), not a flag flip.

**D. Platform shape differences.** The family lock is the clearest example: Linux
queries `CFamilySharing::GetLockingAccount`, Windows has no such class and instead
keeps a `m_mapFamilyRunningApps` map updated by `CUser::UpdateFamilyRunningApps`,
queried through `CUser::GetFamilySharedLockAccount`. Same capability, different
carrier — the port had to find the Windows function rather than translate the
Linux one.

**E. Unverified ABI.** Everything below the planner assumes MS-x64 argument
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
| concurrent play | ✅ | ✅ `family_lock.rs` | ✅ `hooks/family_lock.rs` at the family shared-lock query |
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
   `family_decide.rs` on Linux, both asking `features::family::concurrent`), and
   **hooked** on Windows: `hooks/family_lock.rs` replaces
   `CUser::GetFamilySharedLockAccount`, the query Steam's play-info flags and
   launch result both read.
2. ~~Statistics path~~ — **done as a rewrite.** `decode_job_reply` derives the
   reply field (0x1b0 public / 0x1a8 beta) and body displacement (0x18) by walking
   `BInit`'s instructions; the merge body decodes the response with prost,
   substitutes the rebuilt achievements and re-encodes, restoring Steam's bytes
   afterwards. `Achievements` (and the BST rebuild) now have a carrier on Windows.
3. ~~`CUser::GetAppTicket` anchor~~ — done (both builds), tickets install.
4. ~~Request-code serving~~ — **done.** The body parses the payload as
   `GetManifestRequestCodeRequest`, looks the code up in a bounded process cache
   and swaps the encoded response in for the duration of the original call
   (restoring both slots in `Drop`). Nothing is compiled as an offset: the packet
   layout and the job reply fields are decoded from the target's own code.
5. **Body-per-capability sweep** (class A) against the Linux reference:
   shader control, DLC overrides, callbacks, legacy CD key, SteamID override,
   cloud HTTP, conflict UI.
6. **Linux catch-ups** so the two hosts converge from both sides: wire
   `features::family` + `features::depotcache` into the Linux batches the way the
   Windows host does, and give Denuvo lease mode a caller.
