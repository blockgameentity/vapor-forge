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

---

# Second pass: argument flow, and the two ticket/stats targets

The first pass measured slot *shape* (sizes, store widths, loops, probe scores).
This pass measures the axis the shape could not see — **where each argument
goes** — and adds a second evidence class: MSVC leaves assert and log strings
that name the enclosing method, so a string xref plus "which table holds this
function" gives ground-truth slot names. Both are reported apart from each
other, and neither is a live confirmation.

## What was measured

Tool changes (this branch, `gate0/ticket-stats-anchors`):

* `vtable_pe/msflow/` — a linear **MS-x64** argument walk (`rcx, rdx, r8, r9`).
  `crate::arg_flow` already does this for the System V order, and the existing
  `u32_stores_via_arg` collapses at the first branch, which is why the first
  pass could only *infer* that a store lands in the caller's array. Per slot the
  walk now reports:
  * `thru N:k` — k stores whose address derives from argument N, including
    `mov rax,[r8]` followed by `mov [rax],ecx` (a write into what the argument
    points at). `u32thru` counts the 4-byte ones.
  * `fwd N:k` — k call sites that were handed argument N.
  * `deref N:k` — memory operands reached through argument N, reads included.
  * `arg32 N:k` — reads of argument N as a 32-bit register (`edx`, not `rdx`).
  * `ret R+kb` — returns reached by the walk, and how many of them left `al` a
    0/1 constant or a `setcc` result, i.e. a `bool`.
* `vtable_pe/shape.rs` — the build-shape fingerprint moved out of
  `structure.rs` (file-length rule; behaviour unchanged).
* `vapor-forge-vtable-pe --holds <rva>` — the inverse of an anchor: which
  enumerated tables hold a function, resolving an interior `.pdata` address to
  its entry first (that is what a disassembly cross-reference gives you).

Limits, stated because they bound every claim below: the walk is linear from the
entry and stops at the first `ret` or unconditional jump, so it describes the
straight-line prefix; stack-argument *pointers* are not tracked, so a 5th/6th
argument's out-parameter writes are invisible; caller-saved registers are
dropped at calls, callee-saved ones are kept (an out-parameter is often written
only in the epilogue).

## Method-level labels: string xrefs as ground truth

`objdump -d` over the whole `.text`, grep for the resolved address of a named
string, then `--holds` on the containing function. Public build:

| string | function | holds |
| --- | --- | --- |
| `CUser::GetSubscribedApps` | `0xa0bca0` | **CUser 260-slot 183** |
| `CUser::GetAppLastPlayedTime` | `0xa07360` | CUser 260-slot 158 |
| `CUser::BGetAppMinutesPlayed` | `0x9faf70` | CUser 260-slot 157 |
| `CUser::CheckAppOwnership` | `0xa03a70` | not a slot (unchanged) |
| `CUserStats::GetNumAchievements` | `0xa97010` | CUserStats 56-slot 3 |
| `CUserStats::GetAchievementName` | `0xa95140` | slot 4 |
| `CUserStats::RequestCurrentStats` | `0xa9bde0` | slot 5 |
| `CUserStats::GetNumAchievedAchievements` | `0xa96f10` | slot 40 |
| `CUserStats::GetLastAchievementUnlocked` | `0xa96160` | slot 41 |

Two consequences.

1. **The first pass's `CUser::GetSubscribedApps` candidate is corroborated
   independently.** Slot 183 was picked from the 0x50 licence stride and the
   anchored `CPackageInfo::GetPackageInfo` callee; the function at that slot
   also logs `CUser::GetSubscribedApps`. Two unrelated measurements agreeing on
   that slot is the strongest statement static analysis can make about a *slot*;
   it does not show that the body behaves correctly under a running client (the
   first pass's "What would confirm" for that symbol is unchanged and open).
2. **The `.rdata` method-name tables are usable, but only locally.** The
   `IClientUserStats` name list is in exact vtable order — five independent
   labels above agree with `index - 1051 = slot`, so all 55 `CUserStats`
   methods (0 `GetNumStats` … 54 `BAchievementIconLoaded`) can be named. The
   `IClientUser` list is **not**: its local offset is 30 up to
   `GetAppLastPlayedTime` (index 188 → slot 158) and 28 at
   `GetSubscribedApps` (211 → 183), so two names between them have no slot of
   their own. A name list is therefore evidence per region, never a global map,
   and nothing in this section is a substitute for a live check.

## Target 1 — `CUser::GetAppTicket`

Candidate pool: the four `CUser` tables (356 public slots; 41 / 260 / 48 / 7,
beta 41 / 261 / 49 / 7). Only **7** of the 356 store through argument 3 (the
host ABI's `out`) — six of them in the 260-slot table, one in the 48-slot one —
and the ranking below is over that pool, plus the one `CUtlBuffer` writer for
contrast.

| # | slot | public / beta | size | args (first use) | writes | callees | cross-build |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **1** | **88** | `0xa0a220` / `0xa32810` | 489 / 489 B | `rdx` read as 32-bit, `r8` ptr, `r9` copied then `movsxd` (a size) | `thru 2:1` — `mov byte [r8],0` then `memcpy(dst=r8, src, cbMax)`; `bool` ret (`al` 0/1) | `CBaseUser#33` (`0x518fe0`, the section reader), `CUser#182` (`0x9fcf90`, which calls the anchored `CUser::CheckAppOwnership`), 13 calls | **identical** flow and size |
| 2 | 87 | `0xa16f70` / `0xa3fcf0` | 238 / 238 B | `rdx` 32-bit, `r8` ptr, `r9` unused | `thru 2:2` — zeroes `*r8`, then `*r8 = result` | `CUser#88` (candidate 1 is its only labelled callee), 5 calls | identical |
| 3 | 108 | `0xa0aae0` / `0xa330d0` | 177 / 177 B | `rdx` 32-bit, `r8` ptr written, `r9` ptr, stack arg = size | `thru 2:1` (an 8-byte store) + `memcpy` into `r9`; `bool` ret | none labelled | identical |
| 4 | 258 | `0x9fb930` / `0xa23950` | 576 / 564 B | `r8`, `r9` ptr | `thru 2:1,3:1 u32thru 2:1,3:1` | none labelled | **differs** (`thru 3:1` gone in beta) |

Candidate 1 is the one this branch would defend, and the reason is a string
fact, not a shape fact: `LegacyKeyProofOfPurchaseTicket`, `ProofOfPurchaseTicketRevision`
and `cdk_%u_%u` are each referenced **exactly once in the whole module**, all
three inside `0xa0a220` (`0xa0a323`, `0xa0a3ae`, `0xa0a476`; beta `0xa32810`).
No other function touches the proof-of-purchase (CD-key based ownership) ticket.
The body is exactly "look up this app's ownership ticket, validate its revision
against the engine counter, `memcpy` it into the caller's buffer bounded by the
caller's size, report success".

Candidate 2 is the same path one level up: it probes the ticket into a 0x200-byte
stack buffer through candidate 1 and reports the outcome as `*out = bool`,
returning `1` when the probe ran at all. Candidate 3 reads a per-app 24-byte
entry array at `this+0x3ed8` and writes an 8-byte value plus a copy into two
different caller pointers, with five parameters after `this` — no ticket
strings, so it is a lookup helper in the same neighbourhood rather than the
ticket getter. Candidate
4 fails the cross-build check and carries no ticket strings.

**Rejected, and why — including the obvious-looking one.**

* **slot 20** (`0x597b20` / `0x5b4660`, 170 B, identical in both builds) is the
  only `CUser` slot besides the protobuf serialiser 69 that calls the anchored
  `CUtlBufferEnsureCapacity` (`0xd1b3f0`), and it really does fill a
  caller-supplied `CUtlBuffer`: init, length probe, `EnsureCapacity`,
  fill `buf->m_pMemory`, set the put position, `bool`. Its call looks like
  `(this, app id, const char *, CUtlBuffer *)`. It was the leading candidate
  until its callee chain was read: `CBaseUser#33` (`0x518fe0`) passes its second
  argument to `CBaseUser#34` (`0x519100`), which calls **`CBaseUser` 41-slot slot
  26** (`0x519210`) — a 15-case switch over that argument returning strings
  `news`, `friends`, `apptickets`, `depots_beta`, `depots_dev`, `cache`,
  `streaming_v2`, `Broadcast`, `bigpictureweb`, `nettickets`, … — and builds the
  path `"%s\\%s"` from it. The second argument is a **config section id**, and
  the fourth is a `CUtlBuffer`. That is `GetConfigBinaryBlob`-shaped, not a
  per-app ticket getter. The presence of `"apptickets"` in that switch is a
  namespace, not a ticket body: the string is referenced exactly once in the
  module, at `0x519264`, inside that switch.
* **slot 77**, **181**, **253** — `thru 2:1` but no ticket string, no callee
  into the ticket path, and (181) no `.pdata` entry at all.
* **slot 69** (`0x9fe360`, 734 B) calls `CUtlBufferEnsureCapacity` too, and logs
  `CProtoBufMsg::CProtoBufMsg( EMsg )`: a message serialiser.

**Corpus entry: added.** Both builds share the same 24-byte prologue, so one
pattern serves both; mined with `vapor-forge-pattern-mine` (1 match, unique,
`verified` on the anchor gate):

```
[steamclient."CUser::GetAppTicket"]
follow = "none"
pattern = "48 89 5C 24 10 55 56 57 48 81 EC A0 04 00 00 49 63 E9 49 8B F8 8B F2 41"
```

`--verify` resolves it to `0xa0a220` (public) / `0xa32810` (beta), one match
each, gate verified; the rest of both corpora still resolve (the one
pre-existing `GetPipeClient: unmapped` rejection is unchanged).

**What this entry does *not* prove, and the ABI it implies.** The name is the
host's symbol; the binary does not contain the string `CUser::GetAppTicket`, so
the C++ name is an interpretation of the behaviour. The behaviour itself is
established (unique proof-of-purchase ticket reader, caller-supplied buffer,
size bound, `bool`). The window's argument order is `(this, AppId_t app_id, void
*pTicket, int cbMax) -> bool` — **arg 2 is the raw buffer and arg 3 is a size**,
not a `CUtlBuffer *` and not a `bool`; `crates/host-windows/src/abi/ticket.rs`
assumes the opposite and should be revised with this slot.

**What would confirm it on a live client:** hook `0xa0a220` and log the call
sites — every call should arrive with a small `AppId_t` (the id of a game the
account owns), a buffer, and the buffer coming back with the ticket blob. Its
direct callers are `CUser` slots 87 (the bool-out probe), 90 and 160, plus
`CUser::YieldingRunAllCDKeysFromApp` (`0xa161d0`, named by its own log string) —
which is what a proof-of-purchase reader should be called from. **What would
refute it:** the calls carrying app ids the account does *not* own, or the
buffer being filled with the `cdk_%u` CD-key text instead of a ticket blob, or
`IClientUser::GetAppOwnershipTicketData` being demonstrably this same body — in
which case the entry is misnamed and the host's ticket ABI has to be rebuilt
around slots 108/109 too.

## Target 2 — user stats: class fully named, target shape absent

`CUserStats` RTTI tables: public `0x13b2330` / 56 slots and `0x13b24f8` / 11;
beta 57 and 11. Using the validated name list, every one of the 55
`IClientUserStats` methods is now placed; the slots that hand data back to a
caller are:

| slot | method | size | args | what it does |
| --- | --- | --- | --- | --- |
| 23 | `RequestUserStats` | 496 B | `rdx` CSteamID (8 bytes), `r8` ptr | async request, no caller buffer |
| 24 / 25 | `GetUserStat` ×2 | 493 / 499 B | `rdx` CSteamID, `r8` name ptr, `r9` out ptr | **one** 4-byte store through arg 3, `void` return |
| 26 | `GetUserAchievement` | 87 B | `rdx` CSteamID, `r8` name | three 32-bit stores into three **stack-argument** pointers, `bool` return |
| 17 | `GetUserAchievementProgress` | 792 B | CSteamID, name, two stack out ptrs | same pattern, longer body |
| 42 / 46 | `GetMostRecentAchievementUnlocked` / `GetAchievementAchievedPercent` | 621 / 556 B | | one store through arg 2 |

**No `CUserStats` slot matches the shape the task proposes** ("multiple 32-bit
stores through a pointer, count return"). The closest are 24/25 (a single 4-byte
store, `void`) and 26/17/42 (three separate out-pointers taken on the stack, all
of them written by a callee, `bool` returns). There is no slot that returns a
*count* of stats it wrote into one buffer.

That negative is worth stating precisely, because the selector is not
discriminative even inside `CUser`: slot **257** (`0xa09330`, 430 B, public and
beta identical) has `thru 1:5 u32thru 1:5` — five consecutive 32-bit stores
through argument 2, no calls at all — and it is a **hash-bucket teardown**:
`for (i = count-1; i >= 0; --i) { bucket = base + i*24; bucket[1] = -1;
bucket[0] = i; }` followed by a free-list walk with a 0x58 stride. Five 32-bit
stores through one pointer is what a container destructor looks like.

**No corpus entry.** What evidence is missing, exactly:

* The host's symbol `CUser::GetUserStats` has no counterpart string anywhere in
  either module — unlike `CUser::GetSubscribedApps`, `CUserStats::*` and the
  `IClientUser` name table. There is nothing to anchor a name to.
* `IClientUserStats` has no "all stats for a user into one buffer" method: stats
  are fetched one name at a time (slots 24/25, 26, 17) or asynchronously (23).
  A `GetUserStats` that returns a count of stats written, as the task describes,
  is therefore either a *non-virtual* helper (invisible to the class route, the
  same situation as `CUser::CheckAppOwnership`) or a name for slot 24/25 with a
  looser description than the shape implies.
* The per-user stats block is owned by the callee of 24/25 and 26, which is not
  a `CUserStats` vtable entry (the store through the out pointer happens there,
  not in the slot).

**What would confirm a stats anchor on a live client:** break on the *callee* of
slot 24/25 (`0xa98040` / `0xa97d40` call a shared helper at a fixed RVA) while a
game calls `ISteamUserStats::RequestUserStats` + `GetUserStat`, and check that
the helper walks the per-user stats map and writes 4 bytes per stat; if the
helper is in `.pdata` and has a stable prologue, *that* is the body the
`Achievements` capability should replace, not a `CUserStats` slot. **What would
refute it:** the helper turning out to be the generic `UtlHash` lookup shared
with leaderboards (slots 28-39), which would leave no single interception point
for "all stats of a user".

## Limits of this second pass

* Everything above is static. A string that names a method names the *function
  that logs it*, which is the method in every case checked here, but MSVC inlines
  and merges: the label can belong to an inlined copy.
* The `IClientUser` name table drifts by two entries against the 260-slot vtable
  between index 188 and 211, so it cannot be used to name arbitrary slots; only
  the `CUserStats` list is in exact order.
* `CUser` 260-slot indices are *not* portable between builds on their own (the
  beta table has one extra slot); each slot quoted here was re-measured in the
  beta build and keeps its structure and size.
* Store-through-argument counts are upper bounds on the *prefix*: a body that
  writes its out-parameter only in a late error path is under-counted, and stack
  argument pointers are not tracked at all.
