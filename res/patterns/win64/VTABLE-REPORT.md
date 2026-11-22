# Gate 0 vtable anchoring — four underived symbols

`res/patterns/win64/PROBE-REPORT.md` measured the *bodies* of the four symbols
and shortlisted nothing. This report measures the other axis: the **class
structure**. A class's virtual methods are consecutive function pointers in a
read-only section, in a fixed order per compiler, so an anchored *sibling*
method pins both the class and the slot order a missing method lives in — as
long as the sibling really is virtual and the order survived the rebuild.

Both of those provisos are measured here rather than assumed, and the honest
summary is:

| symbol | vtable anchoring produced | status |
| --- | --- | --- |
| `CUser::GetSubscribedApps` | one credible slot candidate (CUser slot 183) plus one runner-up (slot 40) | **candidate, unconfirmed** |
| user-stats / achievement path | nothing better than the class (`CUserStats`, `CGameStats`) | **no candidate** |
| `CNetPacket` dispatch site | nothing: no packet-class slot is dispatch shaped | **no candidate** |
| `SetEnvString` equivalent | nothing: no `CUser` slot stores a literal `'='` | **no candidate** |

Structural evidence is kept strictly apart from confirmation below. **Nothing in
this report is confirmed**; none of it was checked against a running client.

## What was measured

Tool: `vapor-forge-vtable-pe` (`crates/patterns/src/bin/vapor-forge-vtable-pe/`,
module `crates/patterns/src/vtable_pe/`).

```sh
cargo build -p vapor-forge-patterns --features tools --bin vapor-forge-vtable-pe
PUB=res/patterns/win64/steamclient/669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b.toml
BETA=res/patterns/win64/steamclient/4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10.toml

# anchors that sit in a table, and every other slot of that table
./target/debug/vapor-forge-vtable-pe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --corpus $PUB
# class route (MSVC RTTI names), all slots
./target/debug/vapor-forge-vtable-pe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --class CUser --all-slots
# slot-index stability between builds
./target/debug/vapor-forge-vtable-pe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --corpus $PUB --against /tmp/gate0/publicbeta/steamclient64.dll \
  --corpus-against $BETA
```

| image | build | `.pdata` extents | accepted tables | with RTTI | runs rejected |
| --- | --- | --- | --- | --- | --- |
| `steamclient64.dll` public | 1794536281 | 52,946 | 6,523 | 6,014 | 1,338 (1,337 single-slot, 1 of 5,054 pointers) |
| `steamclient64.dll` publicbeta | 1795048523 | 53,430 | 6,587 | 6,078 | 1,339 (1,338 single-slot, 1 of 5,093 pointers) |
| `SteamUI.dll` public | 1794536281 | 40,826 | 5,376 | 4,887 | 1,034 (1,033 single-slot, 1 of 3,720 pointers) |
| `SteamUI.dll` publicbeta | 1795048523 | 41,159 | 5,392 | 4,901 | 1,037 (1,036 single-slot, 1 of 3,744 pointers) |

A table is accepted when it is a maximal run of 8-byte qwords, each
`image_base + rva` with `rva` inside an executable section, of 2..512 slots.
512 is the sanity bound: the longest plausible vtable here is 261 slots
(`CUser` in the beta build) and the longest qualifying run is 5,054 pointers,
which is a function-pointer array, not a class.

Corroboration is reported but never *required*: how many slots are `.pdata`
function entries (import thunks and `_purecall` stubs are in `.text` but not in
`.pdata`), whether the qword before the table points at data (the MSVC
`RTTICompleteObjectLocator`), and when it does, the RTTI class name. That last
one is what makes the class route possible at all: 6,014 of 6,523 tables in the
public `steamclient64.dll` carry a class name.

## Anchor coverage: vtable anchoring reaches 4 of 23 anchors

An anchor only yields a slot if the anchored function *is* a virtual method. Of
the 14 public `steamclient64.dll` anchors, 13 resolve to a `.pdata` entry
(`GetPipeClient` lands mid-function) and **3 sit in an enumerated vtable**; of
the 9 public `SteamUI.dll` anchors, **1** does.

| anchor | build | rva | vtable base | slot / len | RTTI class | `.pdata` slots | locator before table |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `GetAppDataFromAppInfo` | public / beta | `0x4de420` / `0x4f37e0` | `0x12b8eb0` / `0x12ec270` | **0 / 17** | `CUserAppInfo` | 15/17, 15/17 | yes |
| `IPCProcessMessage` | public / beta | `0x8c46c0` / `0x8e6570` | `0x136c430` / `0x13a10e0` | **16 / 22** | `CIPCServer` | 13/22, 12/22 | yes |
| `CloseAppCloud` | public / beta | `0xa67320` / `0xa916b0` | `0x13ab9a0` / `0x13e1380` | **24 / 95** | `CUserRemoteStorage` | 93/95, 93/95 | yes |
| `CSteamUIAppController::RunFrame` | public only | `0x64ba30` | `0xddc5d8` (`0xe17250` beta) | **2 / 5** | `CSteamUIAppController` | 5/5, 5/5 | yes |

Everything else is a plain (non-virtual) function: `CUser::CheckAppOwnership`,
`CPackageInfo::GetPackageInfo` (`CPackageInfo` has *no* RTTI vtable at all),
`LoadPackage`, `GetDecryptionKey`, `MarkLicenseAsChanged`,
`CUtlBufferEnsureCapacity`, `PchMsgNameFromEMsg`, `BBuildAndAsyncSendFrame`,
`KeyValues_*`, and on the UI side `GetAppByID`,
`BuildCompleteAppOverviewChange`, `FillInAppOverview`, `ShouldShowAppInLibrary`,
`CUpdateManager::MarkAppChange`, `LoadModuleWithPath`, `AddProtobufAsBinary`.

**Consequence for this task:** the route the task proposed — anchor a class
through a verified sibling, read the missing method at a known slot offset — is
unavailable for all four targets, because none of their classes has an anchored
*virtual* method. What is available is the weaker but still useful class route:
the RTTI class name identifies the class's vtables, and their slots are the
candidate pool. That is what is done below.

## Cross-build slot-index stability

Measured by aligning each reference table with the other build's table of the
same class by index and comparing per-slot structural fingerprints (instruction
codes + operand kinds of the first 24 instructions, no displacements or
targets). "Agreement" is slots whose fingerprint is identical at the same index.

| table (public base) | len public → beta | agreement | verdict |
| --- | --- | --- | --- |
| `CUserRemoteStorage` `0x13ab9a0` | 95 → 95 | 95/95 (100%) | stable |
| `CUserAppInfo` `0x12b8eb0` | 17 → 17 | 13/17 (76%) | slot indices stable, four bodies rebuilt |
| `CIPCServer` `0x136c430` | 22 → 22 | 17/22 (77%) | slot indices stable |
| `CUser` `0x139e6e0` | 41 → 41 | 39/41 (95%) | stable |
| `CUser` `0x139e830` | 260 → **261** | 221/260 (85%) | **index changed**: one method inserted, indices shift after it |
| `CUser` `0x139f058` | 48 → **49** | 47/48 (98%) | **index changed** |
| `CUser` `0x139f1f0` | 7 → 7 | 6/7 (86%) | stable |
| `CSteamUIAppController` `0xddc5d8` | 5 → 5 | 4/5 (80%) | stable; the rebuild is slot 2 (`RunFrame` itself, 958 → 1008 B) |

So slot order is *mostly* stable but **not** guaranteed: two of the four `CUser`
vtables gained a slot between these builds, and every index after the insertion
shifts. A slot index is therefore evidence that must be re-checked per build,
which is why the tool reports agreement instead of printing a claimed mapping.

## The four symbols

### 1. `CUser::GetSubscribedApps` — credible candidate, slot 183

`CUser` has four RTTI vtables in the public build: `0x139e6e0` (41 slots),
`0x139e830` (**260** slots), `0x139f058` (48), `0x139f1f0` (7) — 356 slots in
all (beta: 358). A fifth one-slot run at `0x139f1e0` is rejected as too short.

The Linux 64-bit semantic validator for this symbol
(`get_subscribed_apps64_evidence`, `bin/vapor-forge-pattern-scan/semantic.rs`)
requires a license vector and a **`index * 5` `lea` followed by `shl 4`**, i.e. a
0x50-byte license-entry stride, plus a byte (flag) argument. Searching the 356
`CUser` slots for that shape gives exactly four slots, and only two of them
combine it with a loop over 4-byte stores:

| slot | public rva / size | beta rva / size | evidence |
| --- | --- | --- | --- |
| **183** | `0xa0bca0` / 932 B | `0xa34310` / 964 B | 12 `lea [r+r*4]` + 1 `shl 4` (0x50 stride), 8 backward branches, 15 (public) / 17 (beta) 4-byte stores, 3 argument-byte reads, 4 args read in the first 15 instructions (`rcx@0xf rdx@0xb r8@0x7 r9@0x3`), **calls the anchored `CPackageInfo::GetPackageInfo`** (`0x4df9f0` public, `0x4f4c80` beta), no multiplies |
| 40 | `0xa07b30` / 905 B | `0xa2fec0` / 921 B | 11 `lea [r+r*4]`, no `shl 4`, 9 backward branches, 13 4-byte stores, 1 argument-byte read, all four argument registers spilled to home slots and re-read from the stack, calls `CSteamEngine#9` |

Both stay at **slot 40 / slot 183 in both builds** and keep the same structure,
which is the cross-build check passing. Slot 183 is the stronger candidate
because it is the only `CUser` slot in the whole class that iterates with the
0x50 license stride *and* calls an anchored class sibling inside the loop: it
reads a license id, compares it against `-1`, calls a predicate at `0x599140`,
then `CPackageInfo::GetPackageInfo`, and writes 4-byte values inside a nested
loop — a license→app mapping, which is what "subscribed apps" means.

**Structural evidence (measured):** the stride counters, the anchored callee,
the loop and store counts, the argument shape, and slot/index stability across
the two builds.

**Not proven:** that this function is `GetSubscribedApps` rather than another
license enumerator (`GetOwnedApps`-style). Slot 40 is a license enumerator too.
Also, `u32_stores_via_arg` is 0 for both slots: register provenance is dropped
at branches, so "the 4-byte stores land in the caller's array" is *inferred*
from the loop position, **not** measured — the store base is reloaded after
branches and cannot be traced to `rdx` by the current analysis.

**What would confirm:** hook `0xa0bca0` (public) / `0xa34310` (beta) in a live
client and observe it being called with a `AppId_t*`/count pair while a caller
enumerates the account's subscriptions; or find its call site and check that it
allocates `max * 4` bytes and passes a count. **What would refute:** the slot
turning out to be reachable only from inventory/package code paths, or the loop
copying something other than 4-byte app ids.

Note that the behavioural ranking from the probe workstream scores this slot
**0** on the `getsubscribed` profile (it asks for `u32` stores *through argument
2*), while it scores 9 (the maximum) on `userstats` — the profile is not
measuring the right thing here, which is exactly why the class route adds
information the body route did not have.

### 2. User-stats / achievement path — no candidate

Classes reachable by RTTI, public build:

| class | vtables (base / slots) | dispatch-shaped slots | `.pdata` coverage |
| --- | --- | --- | --- |
| `CUserStats` | `0x13b2330` / 56, `0x13b24f8` / 11 (beta: 57, 11) | 0 | 55/56, 6/11 |
| `CGameStats` | `0x12c63d8` / 14 | 0 | 14/14 |
| `IClientGameStatsMap` | `0x1363ab8` / 13 | 0 | 13/13 |
| `CAdapterSteamGameStats001` | `0x133e038` / 13 | 0 | 0/13 (all shared stubs) |

Measured over all 67 public `CUserStats` slots: **no slot stores a 32-bit value
through the second argument** (only slot 21 stores through `this` at all), no slot
stores a literal `'='`, the longest single-register compare chain is 5, and no
slot has a computed branch. The `userstats` probe returns its maximum score (8)
for **22 of 67** slots, so the class route narrows the field from 16,492
functions to 67 slots but the ranking inside those 67 carries almost no
information — the same saturation the probe report described.

The useful part is that the class is now identified: `CUserStats#n` and
`CGameStats#n` are usable as labels, which is what a callee-based search for the
achievement writer needs. Nothing in this measurement names the target.

**What would confirm:** a function whose direct callees resolve to
`CUserStats#n` slots (visible in this tool's `callees` column) together with a
fixed-stride write through `this`. **What would refute:** the path turning out to
be entirely non-virtual, in which case only the caller/callee route will ever
find it.

### 3. `CNetPacket` dispatch site — no candidate

Classes reachable by RTTI, public build:

| class | vtable base | slots | dispatch-shaped slots | note |
| --- | --- | --- | --- | --- |
| `IMsgNetPacket` | `0x142ef88` | 46 | 0 (32/46 `.pdata`) | **31 of 46 slots are the same 26-byte stub at `0xfda860`** |
| `CProtoBufNetPacket` | `0x142f1f0` | 46 | 0 (max chain 2, 15/46 `.pdata`) | |
| `CStructNetPacket` | `0x1430730` | 46 | 0 (13/46 `.pdata`) | |
| `CClientStructNetPacket` | `0x1430ac0` | 46 | 0 (6/46 `.pdata`) | |
| `CCMConnection` | `0x12e32a8` | 10 | 0 (7/10 `.pdata`) | `RecvPkt`'s Linux class; biggest slot is `0x5caf90` (1,489 B, 47 calls, 8 byte stores) but it has no table branch and chain max 84 |
| `CFreeNetPacketsWorkItem` | `0x149a498` | 8 | 0 | |

Not one slot in any packet class contains a jump table or a compare chain over
an `EMsg`-sized constant set. Measured over all 46 slots of each of the four
packet classes: `computed_branch` false everywhere, longest chain 2.

**This refutes a hypothesis rather than confirming one:** the `EMsg` dispatch is
not a virtual method of the packet classes, so vtable anchoring cannot reach it.
Combined with the probe report (3,197 dispatch-shaped functions, none of them a
message switch) the remaining route for this symbol is the EMsg constant table
extracted from `PchMsgNameFromEMsg`, not class structure.

### 4. `SetEnvString` equivalent — no candidate

`CUser::BuildSpawnEnvBlock` is an anchor in the *Linux* corpora only
(`res/patterns/x86*.toml`), and the Windows `CUser` vtables contain no env-block
writer: across the 356 public `CUser` slots, **no slot stores a literal `'='`**
(`equals_imm_stores` is 0 everywhere), and no slot has the `index * 9` license
stride the Linux 32-bit body uses (`stride9_leas` is 0 everywhere). Fifteen slots
combine two or more 1-byte stores with a loop, but none of them stores an
immediate `0x3d`; they are string/serialiser work with 3-20 callees.

As with `CheckAppOwnership`, this is consistent with `BuildSpawnEnvBlock` being a
non-virtual member function: the class route cannot see it. The symbol stays
OPEN, and the only class-derived fact worth carrying forward is that the writer
is **not** among `CUser`'s virtual methods — so searching the `CUser` vtable for
it is refuted, not pending.

## Limits of this measurement

* Slot comparison is structural only. Two builds can keep a slot's shape and
  change its meaning, and a rebuilt body can change the shape without changing
  the method — hence the per-build checks above.
* `u32_stores_via_arg` drops register provenance at every branch and call, so
  "writes through the out pointer" is under-reported in loop bodies (it is 0 for
  the slot-183 candidate even though the loop writes 4-byte values). This is the
  same limitation the probe report lists; it is not new.
* Function extents come from the shared `crate::pdata` module (landed by the
  `gate0/pdata-unify` merge), not from a private copy. Its fragment-merge rule
  keeps more extents than the older tools did — 52,946 against the 49,230 in
  `PROBE-REPORT.md` for the same image — so every size in this report is the
  shared rule's extent and is an upper bound on the body, not a byte count.
* RTTI names come from the mangled type descriptor; template instantiations and
  function-local types are shortened to their first component, so `--class`
  matching is by (case-insensitive) exact-or-substring name.
