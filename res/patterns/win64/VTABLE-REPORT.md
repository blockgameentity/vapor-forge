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

---

# Third pass: the four remaining anchors, mined from names

The first two passes measured slot *shape* and then slot *position*. Both are
blind to a method's identity when its selector has no discriminative shape —
which is exactly the situation the second pass ended in for three of these four
targets. This pass adds the axis that was in the binaries all along: **MSVC
leaves the method name in the module**. Steam logs its own method names, so a
`.rdata` string xref plus "which `.pdata` entry and which table hold the
containing function" is ground truth for identity, and `msflow` plus the
EMsg / RTTI data tables are ground truth for behaviour.

Method (read-only, no network contact, no new dependency): a RIP-relative
reference scan over `.text` (`lea`/`mov` with a `RIP` disp32) maps every string
and every `.data` pointer to its containing `.pdata` extent;
`vapor-forge-vtable-pe --holds` resolves that extent to a slot and an RTTI
class; `vapor-forge-pe-functions --near/--dump` gives extents; `objdump -d`
gives operands; `/tmp/gate0/hints/steamclient/<sha>.toml` (an older Windows
build) gives names, RVAs and signatures; the `.data` EMsg record table
`{char *name, 0, u32 emsg}` gives EMsg → name.

| symbol | verdict | anchor, public / publicbeta | corpus entry |
| --- | --- | --- | --- |
| `CUser::GetSubscribedApps` | **confirmed** | `0xa0bca0` / `0xa34310` — `CUser` slot 183 in both | **added** |
| `CUser::BuildSpawnEnvBlock` | **candidate** (strong) | `0xa1c5d0` / `0xa45520` | **added** |
| `CNetPacket::Dispatch` | **refuted as named**; site located as `CCMInterface::RecvPkt` slot 3 | `0x5d3030` / `0x5f0b50` | **added** under the real name |
| `CUser::GetUserStats` / user-stats slot | **refuted** (shape absent, name absent) | path found: `CUserStats` slot 6 → `CAPIJobRequestUserStats` | none |

Two of the four negatives are the expected outcome, and both are stated with the
measurement that produced them rather than as an opinion.

## 1. `CUser::GetSubscribedApps` — confirmed

Identity and shape agree, from two independent measurements, in both builds.

| evidence | public | publicbeta |
| --- | --- | --- |
| method-name string `CUser::GetSubscribedApps` (`0x13a11c8` / `0x13d66b8`) referenced from | `0xa0beab` | `0xa34535` |
| containing slot | `CUser` 260-slot table, **slot 183** | `CUser` 261-slot table, **slot 183** |
| function extent (shared `.pdata` rule, upper bound) | `0xa0bca0`, 932 B | `0xa34310`, 964 B |
| argument registers read in the first 15 bytes | `r9@0x3 r8@0x7 rdx@0xb rcx@0xf` | identical |
| `msflow` licence stride | `stride5 12 shl4 1` (i\*5 then `<<4` = **0x50**) | `stride5 12 shl4 1` |
| `msflow` argument forwarding | `fwd 1:1,2:1,3:1`, `arg32 0:8` | `fwd 1:1,2:1,3:1`, `arg32 0:8` |
| `msflow` byte-argument reads | `bytearg 3`, one of them `movzbl %r9b,%r15d` at `0xa0bce4` | same at the same offset into the body |

The Linux 32-bit body walks its licence vector with an `index * 9` stride, and
that form is **refuted** for this window: `msflow` reports `stride9 0` for the
slot in both builds. The 64-bit form is what survives, and it survives exactly:
`stride5 12 shl4 1` is the `lea (%rax,%rax,4)` + `shl $0x4` pair, i.e. a
0x50-byte licence-entry stride, which is the shape the Linux **64-bit** validator
for this symbol asks for. The `bool` argument is also present and is the fourth:
`movzbl %r9b,%r15d` at `0xa0bce4` (public) zero-extends `r9b`, and `bytearg 3`
counts three argument-byte reads in the body. Twelve further
`lea (%rcx,%rcx,4)` sites walk the same stride through a base held in `rdi`.

**ABI as measured:** `(this, u32 app id, <out>, bool)` — arg 1 is read as a
32-bit register (`arg32 0:8`), args 1, 2 and 3 are each forwarded to at least
one call, and arg 3 is consumed as the `bool`. The 4-byte stores into the caller
array are still *inferred* from the loop position, not traced: `u32@arg` is `-`
for this slot because register provenance is dropped at the branch, the same
limitation the second pass recorded. The second pass's independent label (the
same string, found from `.rdata` without `msflow`) and this pass's flow
measurement agree on the slot; that is as far as static evidence goes.

**What a live client would show to overturn it:** the function being called with
something other than `(this, app id, array, bool)` — e.g. a CSteamID where the
app id is, or the fourth argument never being 0/1. **What would confirm the
ABI:** a caller that allocates `cMaxApps * 4` bytes and passes that count.

## 2. `CUser::BuildSpawnEnvBlock` — candidate (strong)

The hint corpus names this symbol in an older Windows build:

```
name = "BuildSpawnEnvBlock"   rva = "0x9D2C70"
sig  = "4C 89 4C 24 20 4C 89 44 24 18 48 89 54 24 10 48 89 4C 24 08 55 53 56 57 41 54 41 55 41 56 41 57 48 8D AC 24 B8 FD FF FF"
```

That signature does **not** land in either live build (0 matches; also 0 for the
relaxed `4 homes … lea rbp,[rsp-neg]` form and for the neighbouring
`SpawnProcess` / `RecvPkt` hints — the prologues were all rebuilt). The function
was found by role instead:

| evidence | public | publicbeta |
| --- | --- | --- |
| env-variable name strings referenced by one function (`SteamAppId`, `SteamGameId`, `SteamOverlayGameId`, `SteamAppUser`, `SteamUserDataFolder`, `PATH`, `SteamEnv`, `STEAM_COMPAT_PROTON`, `SteamVirtualGamepadInfo`, 10 `SteamStreaming*`, 5 `SDL_*`, `XR_RUNTIME_JSON`, `http_proxy`/`https_proxy`/`no_proxy`, …) | 63 key-like strings out of 75 string references, inside `0xa1c5d0` | 58 key-like out of 69, inside `0xa45520` |
| source-path assert string in the same function | `…\src\clientdll\user.cpp` at `0xa1d052` | same TU |
| called from | `0xa1e26a`, inside `CUser::SpawnProcessInternal` (`0xa1e050`, named by its own log string `SpawnProcessInternal`) | `0xa47269`, inside `0xa47040` |
| extent / second caller | 6,076 B; also called from `0xa09ac6` (244 B, builds a launch descriptor) | 5,838 B; callers `0xa47269`, `0xa3200a` |
| process creation | none: no `CreateProcess*`/`ShellExecute*` reference in the body, only `tier0_s64!g_pMemAllocSteam` | same |
| entry delta to the spawner | `0xa1e050 - 0xa1c5d0 = 0x1a80` | `0xa47040 - 0xa45520 = 0x1b20` |

**ABI as measured:** `rcx` = `CUser *this`; `rdx` = a launch descriptor read at
`+0x0`, `+0x8`, `+0x10` (64-bit ids), `+0x18` (the `u32` app id that becomes
`SteamAppId=`), `+0x20` and `+0x21` (two `bool`s, `movzbl`); `r8` is forwarded to
the engine's env-builder interfaces (`call *0x50(%rax)` with `rdx = r8`, then
`call *0x80(%rax)` with the assembled block in `r8` and `r8`-the-argument in
`r9`). The block itself is assembled in local string objects and handed over; it
is not returned.

**Why this is a candidate and not confirmed:** the name comes from the older
build's corpus, not from a string in these images. The role — one function, in
`user.cpp`, that materialises the complete child-process environment and is
called by `SpawnProcessInternal` immediately before the spawn — is the role the
hint names, and it is the only function in either module that references all
three of `SteamAppId`, `SteamGameId` and `SteamAppUser` as *keys* (the three
`…=` literals at `0x13a4e60` belong to the *parser* of another process's
environment, `0xa204b0` / `0xa4987a`, which reads them with `strncmp`).

**What a live client would show to overturn it:** a game launch in which
`0xa1c5d0` is *not* called, or in which the environment it hands to the spawned
process lacks `SteamAppId`. **What would confirm the ABI:** hooking it and
logging the descriptor — one call per launch, `+0x18` equal to the launched app
id, and the resulting child environment carrying `SteamAppId=<that id>`.

## 3. `CNetPacket` dispatch — the site is `CCMInterface::RecvPkt`; the name `CNetPacket::Dispatch` is refuted

The first pass concluded that no packet-class slot is dispatch shaped. That is
re-confirmed here with a direct measurement — for each of the 46 slots of
`IMsgNetPacket`, `CProtoBufNetPacket`, `CStructNetPacket` and
`CClientStructNetPacket` (184 slots) the `.pdata` extent contains **0** indirect
or register jumps. The dispatch is not on the packet class; it is one class up:

| `CCMInterface` slot | public | indirect jumps |
| --- | --- | --- |
| 0, 1, 2, 4 | `0x5ca520`, `0x5ca980`, `0x5cb060`, `0x5c5f30` | 0 |
| **3** | **`0x5d3030`, 4,274 B** | **1** |

Slot 3 logs `CCMInterface::RecvPkt` (`0x12e6178` → `0x5d3074`), and its body is
exactly a message dispatch:

* the EMsg comes from the packet through a virtual call — `mov (%rbx),%rax; mov %rbx,%rcx; call *0x60(%rax)` — into `r14d`;
* a first compare handles `0x93` (`k_EMsgServiceMethod` = 147) and `0x92`
  (`k_EMsgDestJobFailed` = 146) plus `0x2bf`, `0x2ef`, `0x157c`, `0x157d`,
  `0x15f8`, and routes them to the job path;
* the rest reaches a real jump table at `0x5d32ab`: `add $0xfffffd0d,%r14d`
  (bias 0x2f3), `cmp $0x7d; ja default`, then the two-level MSVC switch —
  `movzbl byte_table[0x5d4064 + i]`, `mov offset_table[0x5d4038 + case*4],%ecx`,
  `add %rdx,%rcx`, `jmp *%rcx`. One site, in both builds.

The 126 values `0x2f3..0x370` collapse to **9 real case bodies**; 117 values —
including `0x2fd` and `0x370` — reach the shared exit at `0x5d3fce`. The handled
EMsgs are, from the `.data` `{name, 0, emsg}` record table: `0x2f3`
`k_EMsgClientLogOnResponse`, `0x2f5` `k_EMsgClientNotLoggedOnDeprecated`,
`0x2fc` `k_EMsgClientPurchaseResponse`, `0x305` `k_EMsgClientServerList`,
`0x306` `k_EMsgClientGameConnectDeny`, `0x30b` `k_EMsgClientNOP`, `0x311`
`k_EMsgClientGetLegacyGameKey`, `0x32d` `k_EMsgClientSteamUsageEvent`, `0x33e`
`k_EMsgClientClanState`. `CCMConnection::RecvPkt` (`0x5d2f80` / `0x5f09c0`) is a
167-byte forwarder: it takes the packet, and calls `*(this+8)` slot `0x18` —
that is this same `CCMInterface` slot 3.

**Correction to the previous pass:** the "EMsg dispatch is not reachable"
conclusion is wrong in one respect — it is a jump table, but it is over the
0x2f3..0x370 core-message range, not over the whole EMsg space, which is why a
`chain <= 0x4000` ranking never surfaced it (the chain here is a seven-value
pre-check, and 126 range values are dispatched by table). There is also **no
table of handler pointers** anywhere: application messages are not dispatched
here at all — the API that wants a response creates the job
(`CUserStats` slot 6 constructs `CAPIJobRequestUserStats`, section 4) and the job
waits for its own message (`CJob::BYieldingWaitForServiceMethodResponse`,
`0xd39410`).

**Verdict:** `CNetPacket::Dispatch` as a body is **refuted** — `CNetPacket`
subclasses are marshalling objects with no dispatch slot. The dispatch site that
the capability actually needs is `CCMInterface::RecvPkt` (slot 3).

**What a live client would show to overturn it:** an application EMsg reaching a
handler table in a *different* function (that table would be the real dispatch),
or `r14d` at `0x5d3030` not being the wire EMsg. **What would confirm the hook
point:** breaking on `0x5d3030` and seeing every inbound message arrive there
with `r14d` equal to its EMsg.

## 4. user-stats path — the proposed shape is refuted; the path is a CAPI job pair

### 4a. The shape does not exist in `CUserStats` / `CGameStats`

Measured with `msflow` argument tracking over every slot of both classes in the
public build (`CUserStats` 56-slot + 11-slot tables, `CGameStats` 14 slots):

| class | slots | slots with a 32-bit store through an argument | where |
| --- | --- | --- | --- |
| `CUserStats` (`0x13b2330`, `0x13b24f8`) | 67 | **1** | slot 21 (`0xa9d1c0`, 20 insns): `u32thru 0:1` — through **`this`**, not a caller pointer |
| `CGameStats` (`0x12c63d8`) | 14 | **0** | — |

No slot writes N 32-bit values through a pointer argument, and no slot returns a
count of values written. This is not a ranking failure, it is an absence: the
`IClientUserStats` surface is per-stat (`GetUserStat(CSteamID, name, out)`),
per-achievement (`GetUserAchievement`, three stack out-pointers) and async
(`RequestUserStats`), and the aggregate "all stats of a user" path is not a
vtable method at all. The first-pass warning applies: a five-store selector is
not discriminative — `CUser` slot 257 has `thru 1:5 u32thru 1:5` and is a
hash-bucket teardown.

`CUser::GetUserStats` also has **no string anywhere** in either module (unlike
`CUser::GetSubscribedApps`, `CUserStats::*` and the `IClientUser` name table), so
there is nothing to anchor a name to.

### 4b. Where the user-stats path actually lives

Names come from the log strings, functions from the xrefs:

| role | public | publicbeta | how named |
| --- | --- | --- | --- |
| job creator | `0xa92cc0` — `CUserStats` **slot 6** (56-slot table) | `0xabc850` — slot 6 (57-slot table) | allocates `0x198` bytes, constructs with the `CAPIJobRequestUserStats` name string (`0xa92de6`), sets the vtable, registers the job |
| request/response handler + schema writer | `0xa93120`, 2,163 B (schema part starts at a continuation entry `0xa93428`) | `0xabcc20`, 2,043 B | `CAPIJobRequestUserStats - received error/… response failed/missing schema`, `failed to init schema KV`, `updated schema from server` |
| local rebuild / merge | `0xaa03d0` (66 B, returns) then continuation `0xaa0420`, 3,562 B | `0xac97d0`, 3,491 B | `CAPIJobRequestUserStats::YieldingMergeStats - beginning merge/server vs. local/using %s stats` |
| uploader | `0xa902b0` | `0xab9f70` | `CAPIJobStoreUserStats::BYieldingRunAPIJob` |

The merge block is the local-rebuild (BST) point: `… YieldingMergeStats - server
vs. local …` is where server values are reconciled with the client's own. It is
**not anchored here**: `0xaa0420` begins `sub $0x2,%edx; je case2; sub $1,%edx;
je case1; cmp $0xd,%edx; je …` — a coroutine resume block, not a function entry
with a stable prologue, and mining a signature for it would pin a resume state.
The clean, entry-aligned anchors on this path are slot 6 (`0xa92cc0`), the
handler (`0xa93120`) and the uploader (`0xa902b0`).

**Verdict:** `CUser::GetUserStats` as a `CUserStats`/`CGameStats` method is
**refuted**; the capability's real interception points are the `CAPIJob` pair,
and `Capability::Achievements` should be re-pointed at one of them (a
`crates/host-windows` change, outside this corpus-only branch).

**What a live client would show to overturn the negative:** a `CUserStats` slot
that is called with an `AppId_t`, an out array and a capacity while a game asks
for its stats. **What would confirm the replacement:** `0xa93120` being entered
on the `Player.GetUserStats#1` response with the schema protobuf and the local
stats map in hand.

## Corpus entries added and verified

Three symbols, one pattern each, **the same bytes in both builds** (no variant
needed): `follow = "none"`, mined by `vapor-forge-pattern-mine` (1 match,
unique) and accepted by the anchor body gate:

| symbol | pattern | public | publicbeta |
| --- | --- | --- | --- |
| `CUser::GetSubscribedApps` | `48 8B C4 44 88 48 20 44 89 40 18 48 89 50 10 48 89 48 08 55 ?? ?? 48 8D 68 A1 48 81 EC A0 00 00 00 48` | `0xa0bca0` verified | `0xa34310` verified |
| `CUser::BuildSpawnEnvBlock` | `48 89 5C 24 18 48 89 4C 24 08 55 56 57 41 54 41 55 41 56 41 57 48 8D AC 24 ?? ?? FF FF 48 81 EC ?? ?? 00 00 8B 42 18 0F 57 C0 48 8B 1A 4D 8B F8 48 8B 72 08` | `0xa1c5d0` verified | `0xa45520` verified |
| `CCMInterface::RecvPkt` | `48 8B C4 55 48 8D A8 E8 FB FF FF 48 81 EC 10 05 00 00 48 89 58 08 48 8B DA 48 89 70 F0 48 8B F1 4C 89 68 D8 45 33 ED 48` | `0x5d3030` verified | `0x5f0b50` verified |

`--verify` on the full corpus, both images: 18 patterns (public) / 17 (beta)
resolve, the three new ones `verified`, and the only rejection is the
pre-existing `GetPipeClient: unmapped` — unchanged from before this pass.

`CCMInterface::RecvPkt` is a **new symbol name**: the host has no hook body for
it, so the install pass resolves and skips it ("no replacement on this host
yet") rather than failing, and `install::deps` still maps `CNetPacket::Dispatch`
to `CmInterception`. Wiring it up is a one-line `deps.rs` change plus a body.

## Limits of this third pass

* The method-name evidence names the function that *logs* the name. MSVC inlines
  and merges, so a label can belong to the surviving copy of a merged body; for
  every function quoted above the string sits inside the body that the log text
  describes, and slot membership was checked separately.
* `CUser::BuildSpawnEnvBlock` is the weakest of the three entries because its
  name is inherited from an older build's corpus rather than read out of these
  images. Its *behaviour* is established; its *identity* rests on role +
  call-graph position + the key set.
* The EMsg names come from the module's own `.data` record table
  (`{char *name, 0, u32 value}`), whose layout was inferred, not documented. It
  is validated at the two values the dispatch code itself treats specially —
  146 and 147, `k_EMsgDestJobFailed` and `k_EMsgServiceMethod` — and the whole
  `0x2f3..0x370` set is coherent with the client-message range. A name that is
  off by one somewhere in that range changes no anchor and no verdict here: the
  evidence is the jump table and the values in it, not the names.
* Nothing here is a live confirmation. A hook on `0xa0bca0`, `0xa1c5d0` or
  `0x5d3030` in a running client is what would turn these from static evidence
  into fact.

---

# Fourth pass: the seven remaining capability anchors

The third pass worked from *method-name strings*; this pass works from *interface
name tables* plus the argument shapes the capabilities imply. The new ingredient
is that the module carries, for every `.rdata` interface, a run of bare method
names — one per method, in interface order — and each name is referenced by
exactly one function (`lea name; log/forward`). Reading the name's containing
slot out of `vapor-forge-vtable-pe --holds` therefore yields an **interface →
slot → function** map with five independent confirmations on `IClientUser`, for
free. Where that map exists, a target's slot is derived, not guessed; where it
does not (or where the body contradicts the derived slot's ABI), the target is
reported as a negative.

| target | capability | class | slot | anchor (public / publicbeta) | verdict | corpus |
| --- | --- | --- | --- | --- | --- | --- |
| 1 stats | `Achievements` | `CAPIJobRequestUserStats` | 16 / 17 (own override) | `0xa8fd20` / `0xab99a0` (`BInit`) and `0xaa0420` / `0xac97d0` (`YieldingMergeStats`) | **candidate (strong)** — the path is a job chain; these are its run entry and its merge | **added** (2) |
| 2 `GetShaderCacheDepot` | `ShaderCacheControl` | none (plain function) | — | `0x533ad0` / `0x546020` | **candidate (strong)** | **added** |
| 3 `IsAppDlcInstalled` | `DlcOverrides` | `CUserAppManager` | 9 / 116 | `0x4f6a40` / `0x50c740` | **confirmed** (name string + interface order) | **added** |
| 3 `BIsDlcEnabled` | `DlcOverrides` | `CUserAppManager` | 11 / 116 | `0x4eb080` / `0x500ce0` | **confirmed** (interface order + appinfo keys) | **added** |
| 4 `RequiresLegacyCDKey` | `LegacyCdKeyControl` | `CUser` (`IClientUser`) | 87 / 260 | `0xa16f70` / `0xa3fcf0` | **confirmed** | **added** |
| 5 `SetAPICallResult` | `CallbackEvents` | `CSteamEngine` (`IClientUtils`) | 25 / 117 | `0x9bf3d0` / `0x9e50c0` | **candidate** — it is the `…WithoutPostingCallback` half | **added** |
| 5 `RegisterInternalCallback` | `CallbackEvents` | — | — | — | **refuted** | none |
| 6 `GetSteamID` | `SteamIdOverride` | `CUser` (`IClientUser`) | 10 / 260 (derived) | `0xcfed0` / `0xd0470` (derived) | **candidate (weak)** — the slot is derived, the ABI contradicts the Linux contract | none |
| 7 `RegisterJsMethod` | `ConflictUiBridge` | — (`SteamUI.dll`) | — | — | **no candidate** | none |

Five of the seven targets end with a corpus entry; `RegisterInternalCallback`
(inside target 5), `GetSteamID` and `RegisterJsMethod` end in a negative. That
split is what the measurements below produced, not a quota.

## The interface name tables, and how far they can be trusted

`.rdata` holds one run of bare method names per interface version and a second
copy for the next version (for `IClientUser`: `0x134a8xx` and `0x1358xxx`). Each
name is referenced from exactly one `lea`, inside the *one* function that
implements or forwards that method, so a name is a label for a function, not a
position in a list. Position comes from the containing vtable:

```
$ vapor-forge-vtable-pe --dll …/steamclient64.dll --module steamclient --holds <ref>
holds 0x0088f7c5 -> entry 0x0088f6d0: table 0x0135d7b0 slot 25/117 class(IClientUtilsMap)
```

That gave these maps (measured, both builds; only the public numbers are quoted):

| interface | table | names resolved | register |
| --- | --- | --- | --- |
| `IClientUser` | `0x1357d28`, 260 slots | 1 `LogOn`, 2 `InvalidateCredentials`, 3 `LogOff`, 4 `BLoggedOn`, 5 `GetLogonState`, 6 `BConnected`, 7 `BInitiateReconnect`, 8 `EConnect`, 10 `GetSteamID`, 11 `GetClientInstanceID`, 29 `RequestLegacyCDKey`, 69 `BUpdateAppOwnershipTicket`, 87 `RequiresLegacyCDKey`, 88 `GetLegacyCDKey`, 89 `SetLegacyCDKey`, 90 `WriteLegacyCDKey`, 91 `RemoveLegacyCDKey`, 92 `RequestLegacyCDKeyFromApp`, 104-106 `GetAppOwnershipTicketLength/Data/ExtendedData`, 183 `GetSubscribedApps` | 20 |
| `IClientAppManager` | `0x135f708`, 115 slots | 9 `IsAppDlcInstalled`, 10 `GetDlcDownloadProgress`, 11 `BIsDlcEnabled`, 12 `SetDlcEnabled`, 13 `SetDlcContext`, 14 `GetDlcSizes` | 6 |
| `IClientUtils` | `0x135d7b0`, 117 slots | 24 `GetAPICallResult`, 25 `SetAPICallResultWithoutPostingCallback` | 2 |
| `IClientGameServer*` | `0x135a120`, 69 slots | 12 `GetSteamID` | 1 |

The map is *derived* (name → xref → slot), so its correctness is testable, and
two tests were run:

* **Index 4 and 5 are the methods their names say.** `CUser` 260-slot table slot
  4 is `cmpl $0x4,0x178(%rcx); sete %al; ret` (i.e. `m_logonState == 4` —
  `BLoggedOn`) and slot 5 is `mov 0x178(%rcx),%eax; ret` (`GetLogonState`). Both
  are also shared, by linker folding, with `CGameServer` slots 12/13.
* **The `CUser` 260-slot table is `IClientUser` order.** Slot 183 is
  `CUser::GetSubscribedApps` (third-pass label) in `IClientUserMap` slot 183 and
  in `CUser` slot 183; slots 87-92 land on the CD-key cluster described below.
  This is also why 260 slots is not a coincidence: it is the interface length.

Note the adjustor thunks: 110 of the 260 `CUser` slots are not `.pdata`
entries, and many of those (sampled: slots 0, 3, 4, 5, 6, 9) are
`sub $0x1e78,%rcx; jmp impl` thunks — the `IClientUser` subobject sits at
`+0x1e78` in the object, so a member access written against the interface
pointer carries a *negative* displacement such as `-0x1c6e`
(= object `+0x20A`). This is why the members in section 6 look odd; they are
`this_interfaces + 0x1e78 - 0x1c6e`.

## 1. The user-stats path: a job chain, and the merge is the hook

The proposed shape ("a slot writes N 32-bit values through a pointer") is not the
Windows shape, and the third pass was right to refute it. What exists is a
four-stage job, and every stage is named by its own log strings:

| stage | public | publicbeta | named by |
| --- | --- | --- | --- |
| request entry (game → client) | `CUserStats` slot 23 `RequestUserStats` (496 B) | slot 23 | third pass |
| job creation | `CUserStats` slot 6 = `0xa92cc0` (allocates `0x198` B, constructs the job) | `0xabc850` | `CAPIJobRequestUserStats` ctor string |
| **job run: `BInit()` (the class's only non-shared slot)** | **`0xa8fd20`, 736 B, `CAPIJobRequestUserStats` slot 16/17** | **`0xab99a0`, 902 B** | `CAPIJobRequestUserStats - no connection to Steam` |
| request + response parse, schema write | `0xa93120`, 2,163 B | `0xabcc20` | `… - received error '%s' (%d)`, `… - missing schema in response!`, `… - updated schema from server` |
| **merge (server vs. local — the BST rebuild)**, `YieldingMergeStats` | **`0xaa0420`, 3,562 B, entry-aligned** | **`0xac97d0`, 3,491 B** | `CAPIJobRequestUserStats::YieldingMergeStats - beginning merge`, `server vs. local 'score'`, `using %s stats data` |
| upload | `CAPIJobStoreUserStats::BYieldingRunAPIJob` `0xa902b0` | `0xab9f70` | `CAPIJobStoreUserStats::BInit() - …` |

The call graph settles which function is the carrier: `0xa8fd20` calls
`0xa93120` at `0xa8ff72` and `0xaa0420` at `0xa8ff7e`, and nothing else calls
either. In publicbeta the same shape holds: `0xab99a0` calls `0xabcc20` and
`0xac97d0`. So the job's run function validates the session
(`call 0x1a4dc0` on `this+0x190`, the state mask at `this+0x194`, the target
CSteamID at `this+0x198`), sends and parses the reply, and merges it — all in one
entry.

**The third pass's refusal to anchor the merge is withdrawn.** It read
`0xaa0420` as a coroutine resume block. It is not: `0xaa0420` is its own `.pdata`
entry, its extent is 3,562 B and ends in `ret`, and the switch it opens with is
over the **second argument**, which the only caller supplies as an `EResult`:

```
call *0x28(%rax)            ; engine interface: wait for the service-method response
call 0x8dad0                ; BYieldingWaitForServiceMethodResponse(…, &eresult)
test %al,%al
je   → call 0xa93120        ; response parse; leaves the EResult in eax
mov  %edi,%edx              ; edx = EResult  (from 0xa93120, or the waited one)
mov  %rbx,%rcx
call 0xaa0420               ; YieldingMergeStats(this, EResult)
```

so `sub $0x2,%edx; je …; sub $1,%edx; je …; cmp $0xd,%edx; je …` is an
`EResult` dispatch (2 = `k_EResultFail`, 1 = `k_EResultOK`, `0xd`, …), not a
generator state, and the block is an ordinary anchorable function. The 66-byte
`0xaa03d0` before it is the *other* arm the third pass looked at; it is not this
function's epilogue (the extents are separate), and nothing calls it.

**Corpus entries: two, both added.**

* `CAPIJobRequestUserStats::BInit` — `0xa8fd20` / `0xab99a0`, slot 16/17. The
  class's only non-shared slot (0-15 are `CAPIJob` base bodies), `bool`-returning (the early "no connection to Steam"
  path is `xor %al,%al; …; ret`), and it drives both phases. Name is an
  interpretation: the class's virtual that runs the request; the behaviour is
  measured.
* `CAPIJobRequestUserStats::YieldingMergeStats` — `0xaa0420` / `0xac97d0`,
  the completion/merge point and the local-rebuild (BST) site. The name is the
  function's own log string.

### What the Windows hook would have to be (the deliverable for the body workstream)

The Linux `Achievements` hook is a set of `IClientUserStats` accessors read
through one vtable. On Windows that split does not exist:

* **Read path** (`num stats`, `stat name`, `achievement name`, read a stat) is
  available as `CUserStats` (`0x13b2330`, 56 slots) — slots 3 `GetNumAchievements`,
  4 `GetAchievementName`, 5 `RequestCurrentStats`, 40 `GetNumAchievedAchievements`,
  41 `GetLastAchievementUnlocked` are string-confirmed; 23/24/25/26/17 are the
  per-user fetch/getters. These are ordinary entry-aligned functions.
* **The local rebuild (BST)** is *not* a read accessor: it is
  `CAPIJobRequestUserStats::YieldingMergeStats(this, EResult)`, a plain 3.5 kB
  function that runs after the server reply has been parsed. A hook that wants to
  substitute local values must intercept **the job**, not the stats map, and
  there are three entry-aligned sites, in increasing precision:
  1. `0xaa0420` (`0xac97d0`) — `YieldingMergeStats` itself, the completion
     point. A detour here runs *after* the server values are in hand and
     *before* they are merged into the local map; it is the only site that can
     replace the merge outcome without reimplementing the request. The only
     caller ignores whatever the call leaves in `eax`, so treat it as `void`: a
     detour rewrites the merge input (the job's stats block) rather than a return
     value.
  2. `0xa93120` (`0xabcc20`) — the response handler: parses the schema and the
     stats out of the reply and returns an `EResult` that is then fed to the
     merge as `edx`. A detour here can rewrite the reply payload.
  3. `0xa8fd20` (`0xab99a0`) — `BInit`, the job's only override: one detour sees
     the whole request/response/merge cycle; the job object holds the app id
     (`+0x1a0` family), the target CSteamID (`+0x198`) and the stats block.
* `0xaa03d0` is *not* this merge's epilogue (separate `.pdata` extent, no
  caller); anchoring there would never fire.
* Consequence for `crates/host-windows`: `install::deps` gates `Achievements` on
  `CUser::GetUserStats`, which is the read-side name. Both anchors added here are
  on the write/merge side, so the dependency table should gain
  `CAPIJobRequestUserStats::YieldingMergeStats` (and/or
  `CAPIJobRequestUserStats::BInit`) as `Capability::Achievements` dependencies
  rather than re-pointing the existing name.

## 2. `GetShaderCacheDepot` — the key string, and a body that matches the Linux validator line for line

`GetShaderCacheDepot` appears in no string. The **config key it reads** does:
`shadercachedepot`, exactly once per module (`0x12c5598` public, `0x12f8948`
beta), referenced from exactly one function — `0x533ad0` (177 B) / `0x546020`
(177 B). It is *not* a virtual method (`--holds`: "not a slot of any enumerated
table"), so the `CShaderCacheManager` RTTI class route is empty by construction:
`CShaderManager` does have RTTI tables, but this accessor is a plain function.

The body is the Linux 64-bit validator's evidence set, field for field:

| Linux validator asks for | public body |
| --- | --- |
| shader manager readiness: `mov eax,[rax+0x7C]; test eax,eax` | `mov 0x130de5f(%rip),%rax; cmpl $0x0,0x7c(%rax); je →return 0` |
| app-info AppID at `+0x0C` | `mov 0xc(%rdi),%edx` (passed as the app id) |
| game type lookup, argument `2` | `mov $0x2,%r8d` into `0x4dea30(…, "game", "type", appid)` |
| `cmp eax,1` | `call 0x4dea30` → `call 0xd8c500` (string→enum) → `cmp $0x1,%eax; jne →return 0` (app type must be *game*) |
| shader depot lookup, `7` | `mov $0x7,%r8d; lea "shadercachedepot",%r9; call 0x4de950` → returns the depot id in `eax` |

The caller is the clincher: `CGetShaderDepotManifestJob` (`0x988b80`) loads the
appinfo for `[job+0x1a0]` (`call 0x4f5550`), skips apps whose state flags carry
`1`/`0x40`, calls this function at `0x988c17`, and when the result is `0` logs
`"[ AppID %u ] CGetShaderDepotManifestJob skipping because shader depot ID is invalid."`
(`0x12e…`/`0x1388fd0`) and returns `0xb`. The return value is the shader depot id
for the app, which is what the capability needs to answer `0`.

**ABI as measured:** `u32 (CAppInfo *)` — the app id is inside the argument
(`[arg+0xC]`), not passed separately as on Linux. A replacement body therefore
takes one pointer, calls the original, and returns `0` when
`features::depot_key::should_skip_shader_depot` says skip.

**Verdict: candidate (strong).** It is not "confirmed" only because the C++
symbol name is an interpretation: the behaviour, the key, the caller and the
argument shape are all measured.

## 3. DLC — `IsAppDlcInstalled` is confirmed by a name string, `BIsDlcEnabled` by interface order

`CUserAppManager::IsAppDlcInstalled` is one of the few full
`Class::Method` strings in the module (`0x12bd998`), referenced once
(`0x4f6a8b`), and `--holds` puts that reference in
`CUserAppManager` (`0x012bbef0`, 116 slots) **slot 9** at `+0x4b`. The class is
`CUserAppManager`, not `CAppManager`, and it exists in both builds with the same
length and slot indices.

| | `IsAppDlcInstalled` | `BIsDlcEnabled` |
| --- | --- | --- |
| slot | 9 / 116 | **11** / 116 (same index as `IClientAppManager`) |
| public / beta | `0x4f6a40` / `0x50c740` (1,011 B both) | `0x4eb080` / `0x500ce0` (693 B both) |
| strings inside the body | `dlcappid`, `oslist`, `config/oslist`, `manifests` + its own name | `AppManagesDLC`, `DisabledDLC`, `OptionalDLC` |
| shape | reads the app's DLC appids, then the installed manifests, so "installed" = a manifest exists for the DLC | three appinfo config reads that decide whether the DLC counts as enabled; `bytearg 3`, writes through arg 3 (`thru 3:2`) |
| Linux ABI | `(this, u32 app, u32 dlc) -> bool` | `(this, u32 app, u32 dlc, void *) -> bool` — the fourth argument is the pointer the body writes through |

The slot for `BIsDlcEnabled` is inherited, not guessed: `IClientAppManager`'s
facade table (`IClientAppManagerMap`, `0x135f708`) places
`IsAppDlcInstalled` 9, `GetDlcDownloadProgress` 10, `BIsDlcEnabled` 11,
`SetDlcEnabled` 12, `SetDlcContext` 13, `GetDlcSizes` 14 — and slot 9 is
independently pinned by the name string above, so the run is anchored at both
ends. `CUserAppManager` implements that interface, so 11 is `BIsDlcEnabled`; the
`AppManagesDLC`/`DisabledDLC`/`OptionalDLC` reads in the body are the
confirmation that the slot holds a *predicate over the DLC's enabled state*, not
a setter (a setter would write those keys and flush a config store; this body
reads them and returns the result through the caller's byte pointer).

**Verdict: confirmed** for both (identity from a name string and from a
two-ended interface run, plus body contents; no live check).

## 4. `RequiresLegacyCDKey` — the slot map from the ticket pass, reused

The `IClientUser` run places the whole CD-key cluster consecutively:

| slot | method | public function |
| --- | --- | --- |
| 29 | `RequestLegacyCDKey` | facade only (`0x886150`) |
| **87** | **`RequiresLegacyCDKey`** | **`0xa16f70`** (238 B; beta `0xa3fcf0`, 238 B) |
| 88 | `GetLegacyCDKey` | `0xa0a220` (489 B) |
| 89 | `SetLegacyCDKey` | `0xa1bae0` (532 B) |
| 90 | `WriteLegacyCDKey` | `0xa25900` (`"Invalid CDKey method"`) |
| 91 | `RemoveLegacyCDKey` | `0xa157e0` (`"Invalid CDKey method"`) |
| 92 | `RequestLegacyCDKeyFromApp` | `0xa16170` (`AppProofOfPurchaseKeyResponse_t` size assert) |

The body at slot 87 is the Linux validator's contract exactly: the **first
instruction after the prologue is `movb $0x0,(%r8)`** — `*pbHasKey = false` — then
`mov %edx,%edi` (the app id is read as 32-bit), two appinfo-cache calls
(`0x4dd100`, `0x4dda10` with the app id in `edx`), `call 0x4df490`, and finally `call 0xa0a220` (slot 88) with a `0x258`-byte
stack buffer, whose result is reported back through `r8`. `msflow` reports
`thru 2:2` for the slot, which is those two byte stores through argument 2.

The index alignment is corroborated at four independent points (88 = the
proof-of-purchase ticket reader, 90/91 = the two functions that switch on a CD-key
method, 92 = the `AppProofOfPurchaseKeyResponse_t` handler), so slot 87 is not a
lone inference.

**Verdict: confirmed** (measurement: name-table slot + the out-parameter clear
the Linux validator demands + callee identity).

**Correction to the third pass, stated here because it lands in this cluster.**
Slot **88** (`0xa0a220`), shipped in the corpus as `CUser::GetAppTicket`, is
`IClientUser::GetLegacyCDKey`: it is at interface index 88, it is called by
`RequiresLegacyCDKey` (slot 87) with a stack buffer, and the strings it uses
(`LegacyKeyProofOfPurchaseTicket`, `ProofOfPurchaseTicketRevision`, `cdk_%u_%u`)
name the ticket *fields that carry the legacy key*, which is what
`GetLegacyCDKey(appId, char *pchKey, int cbKey)` extracts. The ownership ticket
blob the `TicketOverrides` body wants is the neighbouring interface group
(`GetAppOwnershipTicketLength/Data/ExtendedData`, slots 104-106). The corpus
entry is left in place under its existing name — renaming it would drop the
`TicketOverrides` gate for a symbol `crates/host-windows` already binds — but the
host should treat it as the CD-key reader, not the ticket reader; a
`TicketOverrides` hook needs interface slot 105 (`GetAppOwnershipTicketData`,
facade `0x781400`).

## 5. `CallbackEvents` — the result store is real, the registrar is not there

The engine implements `IClientUtils`: `CSteamEngine` `0x1391158` has **117**
slots (beta 118) and `IClientUtilsMap` `0x135d7b0` has 117, with
`GetAPICallResult` 24 and `SetAPICallResultWithoutPostingCallback` 25 from the
name strings. `CSteamEngine` slot 25 is `0x9bf3d0` (165 B; beta `0x9e50c0`,
165 B), and its body is the store half of the Linux `SetAPICallResult` semantic:

```
mov  %rdx,0x20(%rsp)        ; key = { apiCall, 0, 0, 0 }
lea  0xca0(%rcx),%rcx       ; m_mapAPICallResults
call 0x9ac090               ; FindOrInsert(map, &key, 1)
imul $0x38,%rbp,%rcx        ; record stride 0x38  (the Linux 64-bit stride)
add  this+0xcd0             ; record array
movl $0xffffffff,0x18(%rbx) ; eresult = k_EResultNone
call 0x9ae380               ; memcpy(record+0x20, payload, cbPayload)
```

The Linux semantic's other two clauses are *not* here, and that is the finding:
the `SteamAPICallCompleted_t` (703 = `0x2BF`) post is the caller's job — which is
what the `WithoutPostingCallback` suffix means, and 703 is one of the values
`CCMInterface::RecvPkt`'s pre-check handles specially (third pass). So on Windows
`SetAPICallResult` is not a single function: it is this store plus a posting
call site. **Verdict: candidate** — the added entry is the store, which is the
choke point every API call result passes through, including results the host
itself produces.

`RegisterInternalCallback` has **no string in any of the four modules** and no
one-argument registrar at a virtual or direct-call site: the internal callback
path is `CInternalCallbackManager`, and the only named sites are
`CSteamEngine::PostInternalCallback` (`0x9bac80` public; the label also resolves
inside `CSteamClient` slot 2/83 at `0x9a7710`), `CInternalCallbackManager::ProcessPendingActions`
(`0x8c01c0`), and the `m_InternalCallbackMap` asserts inside a manager
*destructor* (`0x917d20`, whose tail calls `0x8c1270` with the map). No candidate.

## 6. `GetSteamID` — the derived slot holds an out-parameter accessor: candidate, no entry

The interface maps `GetSteamID` to `IClientUser` slot 10 and
`GetClientInstanceID` to 11; slots 4/5 verify the alignment by body. `CUser`
slot 10 is `0xcfed0` (public) / `0xd0470` (beta), 13 instructions in both, and
both builds encode:

```
mov rax,[rcx-0x1c6e]   ; this + 0x1e78 - 0x1c6e = object + 0x20A
mov [rdx],rax          ; store it through the SECOND argument
mov rax,rdx            ; return the pointer, not the id
ret
```

That is a getter with an out-parameter, and it contradicts the contract the
`SteamIdOverride` body depends on (`u64 GetSteamID()` with the id in `rax`; the
Linux validator is literally `mov rax,[rcx+disp]; ret`). Two further measurements
close the door on calling it confirmed:

* the same body is `CGameServer` slot 12 as well — linker-folded, i.e. one body
  shared by two classes' vtables, so the member offset it uses must be valid for
  both objects; that is a property of the layout the two interfaces share, and it
  is worth knowing before anyone hooks it from one side only;
* `CUser::CacheAccountNameAndSteamID` (`0x9e3060`) stores the account's
  `CSteamID` at object `+0xA8` (`mov %r8,0xa8(%rbx)`), and **no function anywhere
  in either module begins `mov rax,[rcx+0xA8]; ret`** (0 occurrences of
  `48 8B 81 A8 00 00 00`, `48 8B 41 A8`, both builds). The trivial getter the
  Linux side hooks does not exist as a separate body in this build; it is
  inlined at every call site.

Slot 11 (`0xa08cb0` / `0xa31200`, `mov rax,[rcx-0x1ce8]; ret`) *is* the shape
`GetSteamID` should have — but the interface order says 11 is
`GetClientInstanceID`, which is also a 64-bit id getter, and nothing static
separates the two beyond the interface map that puts `GetSteamID` at 10.

**Verdict: candidate (weak), no corpus entry.** What is *derived* is the slot
(two interface maps, five alignments); what is *measured* is an ABI that no
`SteamIdOverride` body written for Linux can use: the id arrives through the
caller's pointer and `rax` is that same pointer. A detour would have to read and
rewrite `*arg2` rather than `rax`, and the argument order would differ from
`u64 GetSteamID(this, …)`. Before that is worth doing, one live check settles it:
break on `0xa08cb0` and on `0xcfed0` during logon and see which returns the
account's CSteamID (compare against the id in a logon IPC packet) — the first
would mean the interface map is off by one here and that the trivial getter *is*
present after all, the second means the out-parameter reading is right. No entry
until then: the 13-instruction body leaves nothing but a displacement to key a
pattern on, and overriding an instance id instead of an account id is worse than
having no `SteamIdOverride` anchor.

## 7. `RegisterJsMethod` — no candidate in `SteamUI.dll`

Neither build contains `RegisterJsMethod` or `RegisterJSMethod` as a string. The
only related string is the log line `CreateSharedJSContext:RegisterJSMethods`
(`0xdea140` public), used at `0x687322` inside a JS-context builder whose
subsequent calls (`0x65c680`, `0x65e1c0`, `0x62a850`, `0x561550`) are context
setup — none of them is a four-argument `(name, fn, flags)` registrar. The map a
registrar would insert into is `m_mapJSRegisteredMethods`, and it is referenced
from exactly two functions, both serialisers: the window-state writer at
`0x20ebc0` (which writes `m_strURLToLoad`, `m_strMostRecentURL`,
`m_strPreventNavigateExceptURL`, …) and `CHTMLWindowController` slot 11
(`0x210800`), which writes members through a writer vtable with string keys.

**Verdict: no candidate.** `SteamUI`'s corpus stays at 7 public / 5 publicbeta
patterns; the reverse bridge has no Windows anchor and the capability stays
blocked. **Superseded by the sixth pass** (§3): the registrar is `CHTMLWindow`
slot 6, `0x20d430` / `0x218ef0`, anchored at the end of this file.

## Corpus entries added and verified

Seven entries, one pattern each; five patterns are **the same bytes in both
builds** (the prologues were not rebuilt); the stats job's two functions needed a
variant each. All
were mined with `vapor-forge-pattern-mine` (1 match, unique, anchor gate
`verified`) and the mined output was re-parsed by the shipped scanner
(`round-trip verified`).

| symbol | public | publicbeta |
| --- | --- | --- |
| `IClientAppManager::IsAppDlcInstalled` | `0x4f6a40` verified | `0x50c740` verified |
| `IClientAppManager::BIsDlcEnabled` | `0x4eb080` verified | `0x500ce0` verified |
| `IClientUser::RequiresLegacyCDKey` | `0xa16f70` verified | `0xa3fcf0` verified |
| `CShaderCacheManager::GetShaderCacheDepot` | `0x533ad0` verified | `0x546020` verified |
| `CSteamEngine::SetAPICallResult` | `0x9bf3d0` verified | `0x9e50c0` verified |
| `CAPIJobRequestUserStats::BInit` | `0xa8fd20` verified | `0xab99a0` verified |
| `CAPIJobRequestUserStats::YieldingMergeStats` | `0xaa0420` verified | `0xac97d0` verified |

Counts after the change: public `steamclient` 25 patterns (24 resolve and pass the
body gate), publicbeta 24 (23), `SteamUI` 7 and 5 (unchanged, all verified). The
only rejection in every run is the pre-existing `GetPipeClient: unmapped` in both
builds — the `GetPipeClient` anchor lands mid-function and is unchanged from the
previous passes.

## Naming notes for the host

* `CAPIJobRequestUserStats::BInit`,
  `CAPIJobRequestUserStats::YieldingMergeStats` and
  `CSteamEngine::SetAPICallResult` are new corpus symbol names; all need
  `crates/host-windows` work (a body and/or a `deps.rs` gate) before they can
  carry a capability. They install as unresolved-and-skipped until then, like
  `CCMInterface::RecvPkt` did.
* `CShaderCacheManager::GetShaderCacheDepot` and
  `IClientAppManager::{IsAppDlcInstalled,BIsDlcEnabled}` are the exact names
  `crates/host-windows/src/hooks/{shader,dlc}.rs` already test for, so their
  `decide` halves become installable without a rename.
* `CUser::GetAppTicket` (slot 88) is `IClientUser::GetLegacyCDKey`; see the
  correction in section 4.

## Limits of this fourth pass

* The interface→slot maps are derived from name xrefs, so a name that names an
  inlined copy labels the surviving function. Five independent agreements on
  `IClientUser` (4, 5, 87-92, 183) and two on `IClientUtils`/`IClientAppManager`
  bound the risk; a single point is never treated as a slot.
* Slot indices are per build. `CSteamEngine` grew a slot (117 → 118) and `CUser`
  already grew one (260 → 261) between these builds, so each slot quoted above
  was re-read in the beta build rather than assumed; the ones quoted for beta in
  the table (`0x9e50c0`, `0xab99a0`, `0x500ce0`, `0x50c740`, `0xa3fcf0`,
  `0x546020`) are beta measurements.
* Nothing here is a live confirmation. `0xa8fd20`, `0xaa0420`, `0x533ad0`,
  `0x4f6a40`, `0x4eb080`, `0xa16f70` and `0x9bf3d0` are all static identifications with the
  strings, slots, callees and argument shapes quoted; the "what would confirm"
  question for each is in its section.

---

# Fifth pass: the packet layout family, `GetSteamID`, `RegisterJsMethod`

The fourth pass worked from interface name tables. This pass works from a place
the previous four could not reach: the **module's own assert expressions and
source-file tags**. `CNetPacket` has no RTTI — it is not in any enumerated table,
so no slot map, no class name, no vtable anchor reaches it — but every method in
its cluster asserts through a tag string that is the *source file*
(`…\src\common\netpacket.cpp`) and an expression that names the *member*
(`m_cRef == 0`, `!m_pParsedHeader && !m_pParsedBody`) or the *argument*
(`pubData`, `cubData`). That turns "a function that stores four arguments into
`this`" into "the netpacket.cpp function that asserts `pubData` and `cubData`",
which is the identity the packet layout needed. The other two targets stay
negatives, and both are now refused for a measured reason rather than a missing
route.

| target | verdict | anchor, public / publicbeta | corpus entry |
| --- | --- | --- | --- |
| `CNetPacket::Init` | **confirmed** (role, class and members; the member *name* is an interpretation) | `0xead290` / `0xebf080` | **added** |
| `CNetPacket::Alloc` | **confirmed** | `0xd3fe90` / `0xd6bcd0` | **added** |
| `CNetPacket::Release` | **confirmed** | `0xead4d0` / `0xebf2c0` | **added** |
| `CUser::GetSteamID` | **candidate (strong)** for the slot, **refuted** as an anchor | slot 10, `0xcfed0` / `0xd0470` | none |
| `RegisterJsMethod` | **no candidate** | — (`SteamUI.dll`) | none (anchored in the sixth pass: `0x20d430` / `0x218ef0`) |

## What was measured

Same corpus, same anchor body gate, same `--verify` discipline
(`res/patterns/win64/README.md`). Three additions to the method, all read-only:

* **assert strings as ground truth.** The inline assert path of these functions is
  `call *<assert>` with `r8` = expression, `rdx` = line, `rcx` = source file. A
  RIP-relative reference scan that resolves those three operands to strings gives
  the file and the expression, which names members and arguments.
* **cluster neighbourhood.** A method family sits together in the image (the
  packet methods are all within `0xead080..0xead640`, public), so a log string
  that names one member of the family locates the rest by proximity, and each is
  then confirmed by its own body.
* **hand argument flow.** `crate::arg_flow` (and therefore `msflow`) seeds the
  System V register order, so on these images every "argument N" it reports is
  wrong. The ABI below was read from the disassembly by hand and stated as such;
  section 1.5 measures exactly what the shipped walk does with these bodies.

## 1. `CNetPacket` — the layout, and three anchors

### 1.1 The family, and why it is one family

All of these are inside `…\src\common\netpacket.cpp` (the assert tag at public
`0x13949a3d0`, referenced by `Init`, `AddRef`, `Release` and `SetData`) except
`Alloc`, whose tag is `…\src\common\net.cpp` (`0x13942de68`). Public RVAs; the
beta RVA for each function is in the verdict table above and the corpus table at
the end of the report.

| function | public | size | what its body does |
| --- | --- | --- | --- |
| constructor | `0xeac900` | 27 B | zeroes `+0x1c` (`m_cRef`), `+0x28`, `+0x2c`, `+0x30`, `+0x48` (`m_pParsedHeader`), `+0x50` (`m_pParsedBody`); returns `this` |
| **`Init`** | **`0xead290`** | 514 B | stores four of its arguments into `this` (1.2), asserts `pubData`, `cubData`, `!m_pParsedHeader && !m_pParsedBody`, `m_cRef == 0`, then `m_cRef = 1` when its last argument is set |
| `AddRef` | `0xeac9f0` | 170 B | `incl 0x1c`; when the count rose above 1 and `+0x20` is null it allocates `[+0x18]` bytes, `memcpy`s from `[+0x10]`, and stores the copy at `+0x20` **and** `+0x10`; logs `CNetPacket::AddRef - Copy` (netpacket.cpp line 142) |
| `SetData` | `0xead5e0` | 84 B | frees the previous `+0x20` buffer when it is not the new pointer, then sets `+0x18`, `+0x20`, `+0x10` |
| **`Release`** | **`0xead4d0`** | 262 B | `subl $0x1,0x1c(%rcx)`; on zero it pushes the object onto the module's deferred-free `CUtlLinkedList` (`utllinkedlist.h` asserts) |
| `ReallyRelease` drain | `0xead080` | 419 B | the queue processor: logs `CNetPacket::ReallyRelease() [%s] [%u] - %s` / `[%u] - null`, frees `+0x48`, `+0x50`, `+0x20` |
| **`Alloc`** | **`0xd3fe90`** | 226 B | allocates `0x58` bytes through the allocator interface, calls the constructor, allocates a copy of the payload, `memcpy`, then `Init(packet, 0, copy, size, copy, true)` and returns the packet (1.3) |

The two consumers that make this a *packet* and not an arbitrary object:

* `0xd4bec0` (842 B) — the wrapper factory that `CCMConnection::RecvPkt` calls —
  reads `+0x18` (length), `+0x10` (payload) and `+0x28`, and builds one of the
  `IMsgNetPacket` wrappers around the object. The host's
  `crates/host-windows/src/abi/netpacket.rs` already describes this accessor by
  shape ("reads packet fields at `+0x10`/`+0x18`/`+0x28`"); this pass confirms
  which object those offsets belong to.
* `CCMConnection::RecvPkt` (`0x5d2f80`, 167 B) logs `CCMConnection::RecvPkt()
  multi` and hands the wrapper to `CNetConnectionBuffer::MultiplexMultiMsg`
  (`0xe90940`, 430 B → `0xe90330`), which splits a multi-message into
  sub-packets: it reads the sub-packet length out of the buffer, takes the
  connection id from the connection's own getter (`call *0x20(%rax)`), and calls
  `Init(sub, conn_id, data, size, 0, 1)`. That call site is the Linux
  `CCMConnection` loop's counterpart, and it is where `Init`'s ABI is visible in
  full.

### 1.2 The field offsets, as `Init` writes them

Public `0xead290`; the beta body (`0xebf080`) writes exactly the same offsets.

```
mov  %r9d,%edi             ; cubData
mov  %r8,%rsi              ; pubData
mov  %edx,%ebp             ; unConnID
mov  %rcx,%rbx             ; this
test %r8,%r8   ; Assert( pubData )                      netpacket.cpp:45
test %edi,%edi ; Assert( cubData )                      netpacket.cpp:46
mov  0x80(%rsp),%rax       ; arg 5 (first stack argument)
movups <zero>,%xmm0        ; 16-byte constant from .rdata 0x13949a3a8
mov  %rax,0x20(%rbx)       ; m_pOwned  = arg 5
mov  %edi,0x18(%rbx)       ; m_cubData = cubData
movups %xmm0,0x38(%rbx)    ; +0x38..0x47 zeroed
mov  %ebp,(%rbx)           ; m_unConnID = unConnID
mov  %rsi,0x10(%rbx)       ; m_pubData  = pubData
...
mov  %ecx,0x4(%rbx)        ; from a u32 module global (.data 0x13976f308, 0 here)
mov  %ecx,0x8(%rbx)        ; the same global
cmpq $0x0,0x48(%rbx)       ; Assert( !m_pParsedHeader && !m_pParsedBody )
cmpq $0x0,0x50(%rbx)       ;   (rdi = lea 0x50(%rbx))
xor  %esi,%esi             ; zero
mov  %rsi,0x48(%rbx)       ; m_pParsedHeader = null
mov  %rsi,0x50(%rbx)       ; m_pParsedBody   = null
mov  %sil,0x2c(%rbx)       ; byte field = 0
mov  %esi,0x28(%rbx)       ; dword field = 0
cmp  %sil,0x88(%rsp)       ; the bool argument
je   done
cmp  %esi,0x1c(%rbx)       ; Assert( m_cRef == 0 )
incl 0x1c(%rbx)            ; m_cRef = 1
```

| offset | width | what it is | how it is known |
| --- | --- | --- | --- |
| `+0x00` | 4 | connection id — `Init`'s `edx` (`unConnID`) | `mov %ebp,(%rbx)`, `ebp` = `edx`; `Alloc` passes 0, the multi-splitter passes the connection's own getter result |
| `+0x04` | 4 | u32 module global (`.data` `0x13976f308`, **0** in this build) | `mov %ecx,0x4(%rbx)` after three `movzbl`/`movzwl` loads from that global |
| `+0x08` | 4 | the same global | `mov %ecx,0x8(%rbx)`, same computation |
| `+0x10` | 8 | payload pointer — `Init`'s `r8`, named **`pubData`** by its assert | `mov %rsi,0x10(%rbx)`; read by `AddRef`, `SetData` and the wrapper factory `0xd4bec0` |
| `+0x18` | 4 | payload length — `Init`'s `r9d`, named **`cubData`** | `mov %edi,0x18(%rbx)`; `movslq 0x18(%rcx)` is the `memcpy` length in `AddRef`/`SetData` |
| `+0x1c` | 4 | **`m_cRef`** | `Assert( m_cRef == 0 )` in `Init`; `incl`/`subl $1` in `AddRef`/`Release`; zeroed by the constructor |
| `+0x20` | 8 | owned buffer — `Init`'s first stack argument (the pointer `Alloc` just allocated) | `mov %rax,0x20(%rbx)`; `SetData` frees it; `AddRef` copies into it |
| `+0x28` | 4 | flag/count, zeroed | `mov %esi,0x28`; the wrapper factory `0xd4bec0` tests it (`cmpl $0x0,0x28(%rcx)`) |
| `+0x2c` | 1 | byte flag, zeroed | `mov %sil,0x2c(%rbx)` |
| `+0x30` | 8 | pointer, zeroed by the constructor only | `mov %rax,0x30(%rcx)` in `0xeac900`; `Init` does not touch it |
| `+0x38` | 16 | 16-byte constant from `.rdata` `0x13949a3a8` (zero in this build) | `movups %xmm0,0x38(%rbx)` |
| `+0x48` | 8 | **`m_pParsedHeader`** | `Assert( !m_pParsedHeader && !m_pParsedBody )`, then zeroed; freed by the `ReallyRelease` drain |
| `+0x50` | 8 | **`m_pParsedBody`** | the same assert; freed by the drain |
| object size | `0x58` | — | the constant `edx` of `Alloc`'s allocator call |

The names `m_cRef`, `m_pParsedHeader` and `m_pParsedBody` are read out of the
module; `pubData` and `cubData` name `Init`'s arguments 2 and 3. The names
`m_unConnID`, `m_pubData`, `m_cubData` are the Linux corpus's names for the same
four fields, applied in the same order — the *ordering* is measured here, the
member spelling is not.

### 1.3 `Alloc`, and the object size

```
Alloc(pubData = rcx, cubData = edx)
  lock incl <g_pNetPacketCount>          ; PL5 counter, .data 0x139873214
  mov  $0x58,%edx                        ; object size
  call *0x10(<g_pMemAllocSteam>)         ; allocate, tag = net.cpp
  call 0xeac900                          ; constructor (zeroes 1.2's tail fields)
  cmp  $0xa00000,%esi ; Assert( cubData <= k_cubMaxExpectedMsgDataSize )
  allocate cubData bytes, memcpy(dst, pubData, cubData)
  Init(packet, 0, copy, cubData, copy, add_ref = true)
  return packet
```

Two independent constants agree on the object: the allocation is `0x58` and the
last field `Init` clears starts at `+0x50` (8 bytes). Its callers are a send path
(`0x595bd0`, which looks a connection id up in a `0x20`-stride map, calls `Alloc`,
then wraps the result with `0xd4bec0` and reads the `EMsg` back through the
wrapper's slot `0x60`). Beta: `0xd6bcd0`, constructor `0xebe6f0`, same sizes and
the same `0x58`.

### 1.4 The ABI, as measured

`Init(CNetPacket *this, uint32 unConnID, const void *pubData, uint32 cubData,
void *pOwned, bool bAddRef)` — six parameters, so `rcx, rdx, r8, r9` plus two on
the stack at entry `[rsp+0x28]` / `[rsp+0x30]` (Windows reserves `0x20` bytes of
shadow space). `pOwned` is parameter 5; `bAddRef` is parameter 6 and only
controls the `m_cRef = 1` tail.
`Alloc(void *pubData, uint32 cubData)` takes **no** `this` and returns the packet
in `rax`. `Release(this)` is `void`.

### 1.5 Can `patterns::cnet_packet::decode_init` consume these bodies today? No

> **Superseded by the sixth pass** (§1): the port below was made, and the
> decoder now produces exactly the table at the end of this section. What follows
> is the measurement that motivated it, kept as written.

Measured, not argued: the shipped decoders were run against the three Windows
bodies with `bitness = 64` and the images' own `.text` (a throwaway example, since
deleted):

| decoder | result on the Windows body | first missing fact |
| --- | --- | --- |
| `decode_init` | `Err("connection id store")` | argument 1 is `rdx` here, `rsi` under System V |
| `decode_alloc` | `Err("allocator call with a constant size")` | `args[1]` is read from `rsi`; the size `0x58` is loaded into `edx` |
| `decode_release` | `Err("refcount decrement")` | the decrement is on `rcx`, which the System V seeding labels **argument 3**; `decode_release` looks for `Arg(0)` |

The cause is one layer down from `cnet_packet.rs`: `crate::arg_flow::trace` seeds
`RDI, RSI, RDX, RCX, R8, R9` as arguments 0..5 and clobbers
`RAX,RCX,RDX,RSI,RDI,R8,R9,R10,R11` at every call. On MS-x64 that is wrong twice
over — the `this` pointer is `rcx`, so everything is shifted by three, and
`rsi`/`rdi` are **callee-saved**, which matters concretely here: `Init` holds
`pubData` in `rsi` and `cubData` in `rdi` across the two assert calls
(`call *0x1391b7510`), so a walk that drops them loses both stores. The
`0x100`-byte field cap and the overlap rule in `cnet_packet.rs` are not a problem:
`conn_id = 0` passes `offset % width == 0`, and `0x00/0x10/0x18/0x20` with
`m_cRef` at `0x1c` do not overlap.

**The change this needs** (described, not made — `cnet_packet.rs` is untouched):
make the argument model an ABI parameter instead of a constant.

1. `arg_flow::trace(abi, bitness, code, offset, limit)` with
   `Abi::SysV64 | Abi::Ms64 | Abi::Cdecl32`; `Ms64` seeds `RCX,RDX,R8,R9` as
   arguments 0..3 **and** the first two stack arguments at entry `[rsp+0x28]`,
   `[rsp+0x30]` as arguments 4 and 5 (the same `TRACKED_ARGUMENTS` bound already
   covers it), and clobbers the MS-x64 volatile set
   (`RAX,RCX,RDX,R8,R9,R10,R11`).
2. Thread that ABI through `decode_init`/`decode_alloc`/`decode_release`/
   `decode_api` and the two callers that already know the target ABI
   (`crates/patterns/src/bin/vapor-forge-pattern-scan/semantic.rs`, and the
   runtime's layout decode).
3. Nothing else: `PacketFields`, `spans`, `end`, `overlaps` and the limits are
   ABI-free.

With that, `decode_api` on `0xd3fe90` / `0xead290` / `0xead4d0` would return

```
fields { conn_id: 0x00, data: 0x10, size: 0x18, owned_data: 0x20 }
refcount 0x1c
object_size 0x58
```

which `crates/host-windows/src/abi/netpacket.rs::Layout::new(0x10, 0x18)` accepts
(both slots aligned, disjoint) — i.e. the body workstream's missing `Layout`
becomes derivable from the corpus with no offset hard-coded anywhere.

### 1.6 What confirms this, and what would overturn it

**Confirmed:** class identity from the module's own strings (`netpacket.cpp` tag
plus `CNetPacket::AddRef - Copy` / `CNetPacket::ReallyRelease`), member identity
from assert expressions, the field offsets from `Init`'s stores, the object size
from `Alloc`'s allocation, and the role from the two consumers on the packet path
(the wrapper factory that reads `+0x10`/`+0x18`, and the multi-message splitter
that calls `Init`). Five bodies agree on the same offsets; the beta build stores
the same offsets at the same displacements.

**Not proven:** that the pre-`this` calling convention is the same as the Linux
`Init` (the field *ordering* matches, the argument *count* does too, but the two
extra arguments after `cubData` are `pOwned` and `bAddRef` only by their use);
that `+0x00` is the connection id rather than a sequence number — `Alloc` passes
0 and the splitter passes a connection-layer getter, which is the only
discriminating evidence available statically.

**What a live client would show to overturn it:** a hook on `0xead290` whose
`edx` is not a small connection id, or `Init` being entered with `pubData` equal
to the connection buffer rather than a payload; or `0x58` not being the allocation
size of the object that `0xd4bec0` later reads. **What would confirm the ABI:** a
detour that logs the six arguments for a received packet and sees `cubData` equal
to the wire length and `pOwned` null on the receive path but equal to `pubData` on
the `Alloc` path.

## 2. `CUser::GetSteamID` — the slot is confirmed, the anchor is refused

The fourth pass derived `IClientUser` slot 10 as `GetSteamID` and 11 as
`GetClientInstanceID` from the name table, and refused an entry because slot 10's
body writes through a pointer. This pass adds the field-level proof and the gate
measurement; the verdict stays **no entry**.

| | public | publicbeta |
| --- | --- | --- |
| `CUser` 260/261-slot table slot 10 | `0xcfed0` | `0xd0470` |
| body | `mov -0x1c6e(%rcx),%rax; mov %rax,(%rdx); mov %rdx,%rax; ret` (13 insns, 4 real) | `mov -0x1d3e(%rcx),%rax; mov %rax,(%rdx); mov %rdx,%rax; ret` |
| slot 11 | `0xa08cb0` | `0xa31200` |
| body | `mov -0x1ce8(%rcx),%rax; ret` | `mov -0x1db8(%rcx),%rax; ret` |

**The field slot 10 exposes is a CSteamID — measured.** `CUpdateSubscribedItemsJob`
(`0x9d1300`) reads that same field through the `CUser` pointer it holds and tests
the id's *account type*:

```
mov  0x98(%rcx),%rcx      ; the job's CUser *
add  $0x1e78,%rcx         ; its IClientUser subobject
mov  -0x1c6e(%rcx),%rax   ; = CUser + 0x20A   (the slot-10 field)
shr  $0x20,%rax
and  $0xf00000,%eax       ; bits 52..55
cmp  $0xa00000,%eax       ; k_EAccountTypeAnonUser
```

That is a SteamID64 account-type test and nothing else, so slot 10 is the
account-id accessor even though its ABI is an out-parameter.

**Slot 11 is the instance id.** It is the only two-instruction 64-bit getter in
the 260-slot table (every other no-argument slot there is an adjustor thunk
(`sub $0x1e78,%rcx; jmp`) or a 32-bit getter), which is exactly the shape `crates/hooks/src/client/user.rs` declares
(`unsafe extern "C" fn(*mut c_void) -> u64`, id in `rax`), but it reads a different
field and the interface order puts `GetClientInstanceID` there; its only reader
(`0x9fb890`) formats the value as `%08llx`, not as a SteamID.

**The trivial getter the Linux host hooks does not exist as a body.**
`mov rax,[rcx+0xA8]; ret` has **0** occurrences in either image (byte search for
`48 8B 81 A8 00 00 00 C3` and `48 8B 41 A8 C3`), so `CUser::CacheAccountNameAndSteamID`'s
store at `+0xA8` is only ever read through inlined code.

**Both getters fail the anchor body gate** — the objective reason for no entry.
`vapor-forge-pattern-mine` resolves each body uniquely (`1` match) and then
rejects it as `unmapped` on both builds: neither `0xcfed0`/`0xd0470` nor
`0xa08cb0`/`0xa31200` is a `.pdata` function entry (checked against the raw
`.pdata` table as well: no `RUNTIME_FUNCTION` covers those addresses at all, in
either build — they are leaf thunks with no prologue and therefore no unwind
record). A corpus entry would therefore be a pinned address, precisely what the
gate exists to refuse.

**Verdict: candidate (strong) for the slot, refuted as an anchor.** A
`SteamIdOverride` body on Windows cannot be a two-line `rax` wrapper: it must
detour the out-parameter ABI (`*out = id; return out`) **and** the entry cannot
come from this corpus. The capability needs `crates/host-windows` to publish the
id some other way (the wrapper factory `0xd4bec0` and the packet path already read
`CUser`'s fields through anchored friends) before an anchor question even arises.
**What a live client would show to overturn it:** slot 10 being called during
logon with an `rdx` that comes back holding the account's CSteamID — that would
confirm the accessor *and* the ABI; slot 11 returning the same value as slot 10
would mean the interface map is off by one here.

## 3. `RegisterJsMethod` — still no candidate, but the containers are now named

Neither build contains `RegisterJsMethod` / `RegisterJSMethod` as a string. What
this pass adds is the RTTI the fourth pass did not use: the registration
containers are named in the module, in both builds.

| RTTI name | owner |
| --- | --- |
| `.?AUJSMethodRegistration_t@CHTMLWindow@@` | the registration *value* type |
| `CUtlHashMap<CUtlString, JSMethodRegistration_t@CHTMLWindow, CDefEquals<CUtlString>, HashFunctor<CUtlString>, StringView>` | the map a registrar would insert into, on `CHTMLWindow` |
| `CUtlHashMap<CUtlString, JSRegisteredMethod_t@CHTMLWindowController, …>` | the twin map on `CHTMLWindowController` |
| `CJSMethodCallHandler<CHTMLWindow, …>`, `CHTMLWindow::ExecuteJSCallback` | the call side |

So the missing method is a **non-virtual** member (neither map type has a vtable —
`--class` finds no table for either) of `CHTMLWindow` or
`CHTMLWindowController`, keyed by `CUtlString`. The two classes' tables are 42 + 64
slots (`CHTMLWindow`) and 13 slots (`CHTMLWindowController`); none of their slots
takes `(name, handler, flags)` and inserts into a string-keyed map — the closest,
`CHTMLWindowController` slot 11 (`0x210800`, 602 B), is the window-state serialiser
the fourth pass already identified by its writer-vtable member stores. The only
string in the neighbourhood is still the log line
`CreateSharedJSContext:RegisterJSMethods` (fourth pass: `0xdea140`, used at
`0x687322`), and
`CHTMLWindow::ExecuteJSCallback` (`0xcec590`) is a log-category tag: the function
that references it, `0x20c0b0`, is `CHTMLWindow` slot 1 and logs it at line `0x13`
of its own translation unit.

**Verdict: no candidate.** `SteamUI`'s corpus is unchanged (7 public / 5 beta
patterns). **Superseded by the sixth pass** (§3), which anchors this body from
`CHTMLWindow`'s registration map: `0x20d430` / `0x218ef0`, gate `verified` in both
builds. **What would change it:** a `CUtlString`-keyed insert in either class
whose value stores a `CSmartPtr<CJSCallbackFunc>` plus a flags byte; that shape is
what to search for if a future build keeps its log strings, and the two map type
names above are the handle. **What would overturn the negative:** the JS context
builder at `0x687322` turning out to call such an insert through a thin thunk —
that thunk would be the registrar, and it would need its own anchor.

## Corpus entries added and verified

Three entries, one pattern each; `Init` and `Release` are the **same bytes in both
builds**, `Alloc` differs in exactly one byte (`F0`, a `lock` prefix on the
profiling counter, against `FF`, the indirect call that replaced it).

| symbol | public | publicbeta |
| --- | --- | --- |
| `CNetPacket::Alloc` | `0xd3fe90` verified | `0xd6bcd0` verified |
| `CNetPacket::Init` | `0xead290` verified | `0xebf080` verified |
| `CNetPacket::Release` | `0xead4d0` verified | `0xebf2c0` verified |

Patterns (mined by `vapor-forge-pattern-mine`, 1 match each, uniqueness checked
against each image, `round-trip verified` by the shipped scanner):

```
[steamclient."CNetPacket::Alloc"]
pattern = "40 53 55 56 57 41 56 48 83 EC 30 48 63 F2 4C 8B F1 F0"   # public
pattern = "40 53 55 56 57 41 56 48 83 EC 30 48 63 F2 4C 8B F1 FF"   # publicbeta

[steamclient."CNetPacket::Init"]
pattern = "40 53 55 56 57 48 83 EC 38 41 8B F9 49 8B F0 8B EA 48 8B D9 4D 85 C0 75 28"

[steamclient."CNetPacket::Release"]
pattern = "40 56 48 83 EC 20 83 69 1C 01 48 8B F1 0F 85 ED"
```

`--verify` on the full corpus, both images: all 28 public patterns and all 27
publicbeta patterns resolve, and 27 / 26 of them pass the body gate; the only
rejection in either run is the pre-existing `GetPipeClient: unmapped` (unchanged
from the first pass). Both
`SteamUI` corpora are untouched and still verify. (The sixth pass adds one
`SteamUI` pattern to each build, so those runs are now 8 public / 6 publicbeta — all
`verified`.)

**Naming note for the host:** `CNetPacket::Init`, `CNetPacket::Alloc` and
`CNetPacket::Release` are the three names `crates/hooks/src/client/network.rs`
already resolves and `crates/host-windows/src/hooks/netpacket/availability.rs`
reports as the reason `request-codes` is off. The corpus now anchors them; the
remaining gate is the ABI change in 1.5 and the body work that consumes the
layout, both outside this corpus-only branch.

## Limits of this fifth pass

* The member *names* `Init`, `Alloc`, `Release` are not in the binary. What is in
  it: the source file (`netpacket.cpp` / `net.cpp`), the members `m_cRef`,
  `m_pParsedHeader`, `m_pParsedBody`, the arguments `pubData`, `cubData`, and the
  log lines `CNetPacket::AddRef - Copy` / `CNetPacket::ReallyRelease`. The mapping
  from "the netpacket.cpp function that fills the four fields" to `CNetPacket::Init`
  is the Linux corpus's name for the same role; the role itself is measured.
* The argument flow in section 1.4 is a hand reading. The shipped walk cannot
  confirm it on these images (1.5 measures its failure), so the layout claim rests
  on the literal stores quoted above, not on `msflow`. A corrected walk is the
  cheapest way to re-verify it.
* `+0x04` and `+0x08` receive one module global that is zero in this build, so
  nothing here says what they mean; they are not arguments and no decoder needs
  them.
* The `CUser` field offsets in section 2 are per build: public reads
  `interface - 0x1c6e` (= `CUser + 0x20A` at the fourth pass's `0x1e78` subobject
  offset), beta reads `interface - 0x1d3e`, and beta has no `0x1e78` adjustor at
  all — the object layout moved, so the beta field offset must be re-derived
  rather than assumed.
* Slot indices, sizes and every RVA above were re-measured in both builds; the
  beta column is beta measurements, not a delta applied to public.
* Nothing here is a live confirmation. `0xead290`, `0xd3fe90`, `0xead4d0`,
  `0xcfed0` and `0x210800` are static identifications with their strings, offsets,
  callees and argument stores quoted; each section's "what a live client would
  show" is the check that would turn them into fact.

# Sixth pass: the MS-x64 decoder, and the two negatives re-asked

Three questions, one change of layer: port the packet decoder to the Windows ABI
(§1.5 described the change; this pass made it), then re-ask `GetSteamID` and
`RegisterJsMethod` from angles the fifth pass did not take. The first two stay as
they were, the third **overturns**: `RegisterJsMethod` is now an entry.

## 1. `cnet_packet` decodes the Windows bodies

The change in §1.5 is in, and no longer a description: `arg_flow` takes the
integer-argument convention as a parameter (`Abi::SysV` / `Abi::MsX64`, with the
MS-x64 volatile set — `rsi`/`rdi` callee-saved — and the first stacked argument at
entry `[rsp+0x28]`, past the shadow space), `CodeSegment` carries the container, and
`cnet_packet` gained `decode_*_in(abi, …)` plus `decode_api_segment(&CodeSegment, …)`
which picks the convention from the container. `decode_init`, `decode_alloc`,
`decode_release` and `decode_api` keep their signatures and their System V
behaviour, so the Linux callers are untouched.

Two facts about `Alloc` were not in the §1.5 sketch and are why it needed a window
of its own: the MS-x64 body opens with `DCHECK`s whose failure paths sit between
the entry and the allocator call (0xe1 bytes to the `ret` in public, 0x106 in beta,
against the 0x80 the SysV walk uses), and the beta build steps over a
`lock`-prefixed counter with `jmp` at +0x35, which a walk that stops at the first
unconditional jump never gets past. `Abi::MsX64` therefore walks 0x180 bytes and
follows forward jumps; the SysV walk is unchanged.

Decoded, from a `CodeSegment` of each image, with the corpus patterns resolved
first (1 match each, at the RVAs the fifth pass measured):

| | public | publicbeta | measured (`§1.2`/`§1.3`) |
| --- | --- | --- | --- |
| `CNetPacket::Init` | `0xead290` | `0xebf080` | same |
| `CNetPacket::Alloc` | `0xd3fe90` | `0xd6bcd0` | same |
| `CNetPacket::Release` | `0xead4d0` | `0xebf2c0` | same |
| `conn_id` | `0x00` | `0x00` | `0x00` |
| `data` (`pubData`) | `0x10` | `0x10` | `0x10` |
| `size` (`cubData`) | `0x18` | `0x18` | `0x18` |
| `owned_data` | `0x20` | `0x20` | `0x20` |
| `refcount` (`m_cRef`) | `0x1c` | `0x1c` | `0x1c` |
| `object_size` | `0x58` | `0x58` | `0x58` |

The decoder agrees with the hand measurement on every field, in both builds, and
`crates/host-windows/src/abi/netpacket.rs::Layout::new(0x10, 0x18)` accepts the
result. The golden test that asserts this table runs only when
`VAPOR_FORGE_TEST_STEAMCLIENT` points at a `steamclient64.dll`; unset, it skips. Its
corpus lookup is by the image's sha256, so a build without a corpus file fails
loudly rather than decoding a different function.

## 2. `CUser::GetSteamID` — the ranking, measured over all 260/261 slots

Fifth pass refused the entry on slot 10 (an out-parameter accessor) and noted slot
11 (`mov rax,[rcx-0x1ce8]; ret`) as the shape `SteamIdOverride` wants. This pass
ranked the *whole* table instead of the two derived slots: all slots of the
`CUser` table (`0x139e830`, 260 slots public; `0x13d3b08`, 261 beta — 150/151 of
them `.pdata` functions), each body disassembled from its entry, scoring "no call,
loads 8 bytes from a member of `rcx`, returns it in `rax`, body < 40 bytes".

| slot | public | publicbeta | body | gate |
| --- | --- | --- | --- | --- |
| 10 | `0xcfed0` | `0xd0470` | `mov rax,[rcx-0x1c6e]; mov [rdx],rax; mov rax,rdx; ret` | `unmapped` |
| 11 | `0xa08cb0` | `0xa31200` | `mov rax,[rcx-0x1ce8]; ret` | `unmapped` |
| 57 | `0xa28ce0` | `0xa52c50` | `mov rax,[rcx-0x1b50]; ret` | `unmapped` |
| 213 | `0xa0c1f0` | `0xa34880` | `mov rax,[rcx+0x4790]; mov eax,[rax+0x20]; ret` | `unmapped` |
| 214 | `0x9fe340` | `0xa26110` | `mov rax,[rcx+0x4790]; movzx eax,byte [rax+0x28]; ret` | `unmapped` |
| 250 | `0x562cc0` | `0x57f710` | `mov rax,[rcx+0x4b98]; ret` | `unmapped` |

Six slots have the shape, at the same indices in both builds — and **all six are
leaf functions with no unwind record**, so `.pdata` does not cover them and
`pdata::body::verify` answers `unmapped` for every one. The gate cannot represent a
leaf body at all, so no slot of this table is a corpus entry whatever its shape is;
that is a property of the gate, not of these six. The corpus check agrees: the
8-byte pattern for slot 11 (`48 8B 81 18 E3 FF FF C3`) resolves **uniquely** on
public and is then rejected `unmapped`, which is exactly the fifth pass's verdict
reached from the other end.

One more measurement closes the "maybe the positive-offset form exists" question
that fifth pass answered only for `+0xA8`: `mov rax,[rcx+0x20A]; ret`
(`48 8B 81 0A 02 00 00 C3`) — the account-id field, reached from a `CUser *`
instead of through the `IClientUser` subobject — occurs **0 times in both builds**.
So the id is exposed as a body only through slot 10's out-parameter thunk.

**Verdict: still no entry, and now for a measured reason that covers the table
rather than a slot.** `SteamIdOverride` on Windows needs `crates/host-windows` to
read `CUser`'s field some other way; the anchor question cannot be answered by this
corpus on either build.

## 3. `RegisterJsMethod` — anchored, from the registration map back to its caller

Fifth pass searched for the string and stopped: neither image has
`RegisterJsMethod` or `RegisterJSMethod`. Working back from the *container* instead
finds it. Both builds agree at every step.

**The string that exists is a log tag, not the method.** The only `RegisterJS…`
byte sequence in either image is inside `CreateSharedJSContext:RegisterJSMethods`
(`0xdea140` public / `0xe24ee8` beta, `SteamUI.dll`). It has exactly one reference,
at `0x687322` / `0x6a31e7`, and that reference is a severity-5 argument to the same
logging thunk the sibling tag `CreateSharedJSContext:BuildStartURL` uses — a
`DCHECK` message, which is why searching for it never yields a function.

**The containers are real and their type names pin the value.** `.data` carries
`JSMethodRegistration_t@CHTMLWindow`, the map
`CUtlHashMap<CUtlString, JSMethodRegistration_t@CHTMLWindow, CDefEquals<CUtlString>,
HashFunctor<CUtlString>, StringView>`, the interface `IJSMethodCallHandler`, and
five `CJSMethodCallHandler<…>` instantiations. Following MSVC RTTI through the
32-bit-RVA `COL` layout gives vtables for the handler classes and for
`CHTMLWindow` (`0xcebed0` + `0xcec028` public, 42 + 64 slots; `0xd234a0` +
`0xd235f8` beta), and **no** `COL` for the map type or for
`JSMethodRegistration_t` — those are the containers, and neither has a vtable,
which is the fifth pass's finding restated from the RTTI side.

**The registrar is `CHTMLWindow`'s slot 6, and it is virtual.** Each site that
constructs a `CJSMethodCallHandler<CHTMLWindow, …>` (public `0x2076f5`,
`0x20c547`) then registers it with the same virtual call:

```
mov  rdx,<name string>        ; "Browser.RegisterForOpenNewTab", "Browser.RegisterForGestureEvents"
mov  r9d,0x2                  ; flags
mov  r8,<new handler>         ; the CSmartPtr just built
mov  rcx,<CHTMLWindow *>      ; this
mov  rax,[rcx]
call qword ptr [rax+0x30]     ; slot 6
```

Slot 6 is `0x20d430` (public) / `0x218ef0` (beta), both `.pdata` functions of
exactly **830 bytes**, and their bodies are the same hash-map insert: bucket count
`-1` from `[map+0x10]`, bucket array `[map]`, `0xffffffff` empty sentinel, node pool
`[map+0x30]` with node stride `0x28`, key hash by `call 0xc54ea0` then the mix at
`0x8841f0` (seed `0x417`), key compare through `0xc54e20` against the empty
`CUtlString` sentinel, and on insert `mov [node+0x8],handler` /
`mov [node+0x10],flags`. The map is at `this+0xa0` in public, `this+0x98` in beta.
Four arguments — `(this, name, handler, i32 flags)` — which is byte-for-byte the
Linux detour's `RegisterJsMethodFn`.

| | public | publicbeta |
| --- | --- | --- |
| `CHTMLWindow` slot 6 | `0x20d430` | `0x218ef0` |
| `.pdata` extent | 830 B | 830 B |
| gate | `verified` | `verified` |
| corpus match count | 1 | 1 |
| map member | `this+0xa0` | `this+0x98` |

Both corpus files gained one entry; each build's pattern carries its own map
displacement (the only byte that differs), which is the same policy the
`CNetPacket::Alloc` pair already follows:

```
[steamui."RegisterJsMethod"]
pattern = "48 89 5C 24 18 44 89 4C 24 20 55 56 57 41 54 41 55 41 56 41 57 48 83 EC 40 4C 8D B1 A0 00 00 00 41 8B F9 41 83 7E 44 00 4D 8B E0"   # public
pattern = "48 89 5C 24 18 44 89 4C 24 20 55 56 57 41 54 41 55 41 56 41 57 48 83 EC 40 4C 8D B1 98 00 00 00 41 8B F9 41 83 7E 44 00 4D 8B E0"   # publicbeta
```

`vapor-forge-pattern-mine --verify` on the two `steamui` corpus files: 8 public
patterns and 6 publicbeta patterns resolve, every one `verified`.

**What is measured and what is inferred.** Measured: the slot, the RTTI chain, the
map insert, the `(name, handler, flags)` call with a `CUtlString`/`char *` key at
two construction sites, the extent and the gate verdict, in both builds. Inferred:
the `RegisterJsMethod` *name* for that body — neither image carries it, and the
inference rests on the shape (a named registration into
`CUtlHashMap<CUtlString, JSMethodRegistration_t>`) plus the argument list the Linux
detour already documents. What would overturn it: a caller passing that slot
something other than `(name, handler, flags)`, or a second string-keyed map with
the same insert shape. What a live client would show: a breakpoint on
`0x20d430` firing with `rdx` = `"Browser.*"` during `CreateSharedJSContext`.

## What the two host-side decisions still need

Nothing here changes `crates/host-windows`: `CUser::GetSteamID` is still
`replacement_address(..).is_none()` (no anchor is possible through this gate), and
`RegisterJsMethod` now resolves through the corpus, so the host's
`replacement_address("RegisterJsMethod").is_none()` assertion and the
`ConflictUiBridge` availability reason in `crates/host-windows/src/hooks/mod.rs`
are upstream's to update — this branch is corpus-only.

## 8. The job reply fields — derived, not tabulated

`crates/patterns/src/job_fields.rs` decodes the reply field and body
displacement from `CAPIJobRequestUserStats::BInit`'s instruction stream. The
shape the pass above read by hand is one call deep: `BInit` hands the job
(`this`) to the response parser and the parser dereferences the reply. The
decoder follows the direct calls that receive `this`, follows both arms of a
branch (the parser call sits behind `test al,al; jz …; mov …; jmp …`), tracks
which register holds `this` and which pointer member was just loaded from it, and
requires a `lea rdi|rsi, [that-pointer + disp]` within 0x40 bytes.

| | public | publicbeta |
| --- | --- | --- |
| `BInit` (corpus anchor) | `0xa8fd20` | `0xab99a0` |
| parser the walk follows | `0xa93120` | `0xabcc20` |
| `decode_job_reply` → `reply_field` | **`0x1b0`** | **`0x1a8`** |
| `decode_job_reply` → `body` | **`0x18`** | **`0x18`** |

Both values are read from the instruction stream (the decoder has no per-build
branch); the measured table above is only what the golden test asserts against.
Rejected shapes are rejected with a reason: `reply field load`,
`reply body outside the window`, `ambiguous reply field`,
`reply field outside the job object`.

## 9. `CUser::GetSteamID` — the slot ranking, re-run under the anchor gate

The ranking asked for here — no `call` in the body, a single 64-bit member load
from `this`, `ret` within 0x40 bytes — over the four `CUser` tables returns
three slots in the 260/261-slot `IClientUser` table, and their members resolve to
the same absolute `CUser` offsets in both builds once the secondary-base
displacement is measured (`add $0x1e78,%rcx` public, `$0x1f48` beta, read out of
the account-type test in `CUpdateSubscribedItemsJob`):

| slot | public | publicbeta | body | member |
| --- | --- | --- | --- | --- |
| 10 | `0xcfed0` | `0xd0470` | `mov -0x1c6e(%rcx),%rax; mov %rax,(%rdx); mov %rdx,%rax; ret` | `CUser+0x20A` (account CSteamID, account-type tested) |
| 11 | `0xa08cb0` | `0xa31200` | `mov -0x1ce8(%rcx),%rax; ret` | `CUser+0x190` (instance id) |
| 57 | `0xa28ce0` | `0xa52c50` | `mov -0x1b50(%rcx),%rax; ret` | `CUser+0x328` |

The primary `CUser` 41-slot table repeats slot 10's body at slot 4 (`0xcfec0`,
`mov 0x20a(%rcx),%rax; mov %rax,(%rdx); mov %rdx,%rax; ret`). So the account id
has exactly one accessor shape, and it takes a pointer argument: **no
`this`-only 64-bit getter of the account CSteamID exists in either image** —
`48 8B 81 0A 02 00 00 C3` and `48 8B 81 A8 00 00 00 C3` / `48 8B 41 A8 C3`
(`CSteamID` at `+0xA8`, written by `CacheAccountNameAndSteamID` `0x9e3060`
`mov %r8,0xa8(%rbx)`) both have **0** occurrences, public and beta.

The member both `CheckAppOwnership` (`[r12+0x23e8]`, twice) and
`GetSubscribedApps` (`[r13+0x570]`, its secondary-base frame — the same member)
read is an 8-byte pointer that is dereferenced at `+0x40` and used to walk the
license set. No `CUser` slot is a 64-bit getter of it (`mov rax,[rcx+disp]; ret`
over the whole public image has no `+0x570`), so the discriminator rejects both
this-only
getters: slot 11 formats its member as `%08llx` in its only reader, slot 57 is
not the account id, and slot 10 is the account id but not this-only.

**Verdict: no corpus entry, re-confirmed on both builds.** Even the body gate
alone refuses: `vapor-forge-pattern-mine --verify` resolves each candidate
uniquely and then reports `unmapped` —

```
public    CUser::GetSteamID  OK  1  0x0000a08cb0  unmapped
publicbeta CUser::GetSteamID OK  1  0x0000a31200  unmapped
```

(neither address is covered by a `RUNTIME_FUNCTION`). The same two runs report a
pre-existing failure in the shipped corpus, unrelated to this pass:
`GetPipeClient` at `0x8c50b0` public / `0x8e6f70` beta is also `unmapped`.

## 10. The leaf gate and `CUser::GetSteamID`

The anchor gate used to require a `.pdata` `RUNTIME_FUNCTION` entry whose body
contains a `RET` or ends in a tail jump. A leaf function calls nothing, needs no
unwinding and therefore has no `RUNTIME_FUNCTION` at all, so the gate refused
patchable code purely for being leaf: `CUser::GetSteamID` resolved **uniquely**
in both builds and was then rejected as `unmapped`. The gate now accepts a
non-`.pdata` address on positive evidence (uniqueness from the matcher, `0x0c`
bytes of `0xCC`/`0x90` padding before it, a `RET` within the first `0x40`
bytes), reports the result as `leaf` rather than `ok`, and still answers from the
`.pdata` branch first so a mid-function hit keeps failing. See
[`README.md`](./README.md#leaf-functions-the-gates-one-relaxation) for the rule
and the unit tests (`crates/patterns/src/pdata/body/tests.rs`).

### The walk

`vapor-forge-vtable-pe --class CUser --all-slots` enumerates four `CUser` tables
per build; the primary one is 41 slots and its slot 4 is the out-parameter
accessor the interface map calls `GetSteamID` (slot 10 of the 260-slot
`IClientUser` table is the same body behind the secondary base, which is why its
displacement differs):

| channel | 41-slot table | slot 4 | body | `.pdata` | gate |
| --- | --- | --- | --- | --- | --- |
| public | `0x139e6e0` | `0xcfec0` | `48 8B 81 0A 02 00 00 48 89 02 48 8B C2 C3` | none | `leaf` |
| publicbeta | `0x13d39b8` | `0xd0460` | same 14 bytes | none | `leaf` |

Both entries are preceded by exactly `0x0c` bytes of `int3` — the patch length the
gate asks for — and the body is `mov 0x20a(%rcx),%rax; mov %rax,(%rdx); mov
%rdx,%rax; ret`: a leaf with an out-parameter, as §6 established.

### The signature

Derived by decoding that body, not by copying a literal: the decoder yields four
instructions; wildcarding their operand fields (`48 8B 81 ?? ?? ?? ?? 48 89 02 48
8B C2 C3`) matches **3** sites in public and **5** in publicbeta, so the member
displacement is identity-bearing here and the shipped pattern keeps it. The two
builds encode the slot-4 body **identically**, so no variants are needed; the
slot-10 body is the build-dependent one (`-0x1c6e` public, `-0x1d3e` beta) and
fails the gate anyway. Both corpus files now carry, under the symbol name
`CUser::GetSteamID`, the 14-byte pattern above; `--verify` on both images:

```
public     CUser::GetSteamID  OK  1  0x00000cfec0  leaf
publicbeta CUser::GetSteamID  OK  1  0x00000d0460  leaf
```

`GetPipeClient` is still rejected on both builds (`0x8c50b0` / `0x8e6f70`,
`unmapped`): only 7 `0xCC` separate it from the `lea`/`ret` before it, short of
the patch length, and the matched bytes are a mid-body `test edx,edx`. The
relaxation did not open that hole.

## 11. The family lock — no Windows carrier

Linux's `FamilyLockBypass` carrier is `CFamilySharing::GetLockingAccount(this,
u32 appid) -> u32`. Neither `steamclient64.dll` build has that class:

* **RTTI.** Enumerating every table whose `RTTICompleteObjectLocator` class name
  contains `Family` finds the **same 68 classes in both builds**, and all 68 are
  19-slot PBO messages (`CFamilyGroups_*` requests/responses/notifications) or
  value types (`FamilyGroupMember`, `FamilyGroupPendingInvite`,
  `FamilyGroupFormerMember`, `FamilyGroupPendingInviteForUser`,
  `FamilyGroupMembership`). No `CFamilySharing`, no sharing/lock manager.
* **Cross-build structure.** The 68 tables have an identical per-slot sharing
  profile in both builds (distinct bodies per slot: 68, 1, 68, 68, 30, 1, 1, 1,
  39, 5, 39, 39, 1, 68, 68, 1, 1, 5, 68) and the same 1292 slots — a message
  class hierarchy, not logic that survives as a candidate.
* **AppId slots.** The only slots in those tables that take a 32-bit second
  argument are the five `mov [rcx+disp], edx; ret` field setters at slot 17;
  17 distinct bodies in the 68 tables read `edx`/`rdx` at all, and every one is
  either such a setter or a message clear/merge helper that reads its own field
  at `[rcx+0x10]`. **No slot takes an AppId and returns an account id** — the
  ranked candidate list is empty.
* **Strings.** `FamilySharing`, `GetLockingAccount` and `locking account` occur
  **0** times in both modules; no export (41 in each) names family, sharing or
  licences. The family tags name `CUser` state instead of a missed class:
  `CUser::UpdateFamilyRunningApps`, `m_mapFamilyRunningApps`, `m_mapFamilyDLC`,
  `m_ulFamilyGroupID`, `m_strFamilyGroupName`, `m_strFamilyGroupCountry`,
  `m_vecFamilyGroupMembers`, plus server-sent borrowed-licence fields
  (`borrower_id`, `is_borrowed`, `k_ESharedLibrary_LicenseExcluded_FlagBorrowed`,
  `CClientJobUpdateAuthorizedBorrowers`).

**Verdict: no corpus entry — the lock has no anchorable Windows carrier.** The
Windows client holds family-group state in `CUser` and borrowed-licence flags
from the server; it has no `CFamilySharing` equivalent, no class or slot shaped
like an AppId-to-account-id lookup, and nothing whose structure survives into a
candidate. Not "unanchored yet": absent. A `FamilyLockBypass` on Windows would
have to be written against whatever consumes those borrowed-licence flags, which
this measurement did not find as a method.

**Superseded in part by the eighth pass below.** The search above looked for one shape (a
class with an `AppId -> account` slot) and stopped at the message classes. Windows
makes the same decision in `CUser` instead: `m_mapFamilyRunningApps` holds the
per-app lock record, and `CUser::GetFamilySharedLockAccount(this, appid) -> u32`
is the `AppId -> account` query both of Steam's own lock consumers call. It is
anchored in both live builds. Everything measured above still holds — there is no
`CFamilySharing` on Windows — but the capability is **not** retired; see the eighth
pass at the end of this report.

# Seventh pass: the work-item post, and why this host does not use it

`NativeResponseDelivery` on the Linux host is a **queue**: a fabricated response is
posted to Steam's own network thread with `CWorkThreadPool::AddWorkItem`, and the
posted `CWorkItem`'s `execute` re-enters Steam's `RecvPkt` with a packet built by
`CNetPacket::Alloc`/`Init` from a dispatch context (receiver + connection id)
captured off a real inbound packet. The Linux corpus anchors the post site
(`CWebSocketConnection::PostDelayedCloseWorkItem`, `res/patterns/x86_64.toml:100`)
and derives the pool slot, item size and vtable from its body
(`crates/patterns/src/work_item_site.rs`).

The question for Windows was whether the same post can be anchored. **It can be
described, but not verified end to end, and this host does not need it** — the
answer is delivered synchronously inside Steam's own dispatch. This section
records the measurement so the next pass does not repeat it.

## What was measured

Tools, on both live builds:

```
vapor-forge-vtable-pe --dll <steamclient64.dll> --module steamclient --class CWorkItem
vapor-forge-vtable-pe --dll <steamclient64.dll> --module steamclient --class CWorkThreadPool
vapor-forge-vtable-pe --dll <steamclient64.dll> --module steamclient --class Work   # substring
strings -a -n 6 steamclient64.dll | grep -i workitem
```

The mechanism exists, with stable RTTI tables:

| class | public | publicbeta | slots | `.pdata` |
| --- | --- | --- | --- | --- |
| `CWorkItem` | `0x11f5448` | `0x1225578` | 8 | 5/8 |
| `CWorkThreadPool` | `0x1459290` | `0x148eee0` | 3 | 3/3 |
| `CInternalLambdaWorkItem` | `0x11f5c18` | `0x13ce5d0` | 8 | 3/8, 2/8 |
| `CWebSocketConnectionDelayedCloseConnectionWorkItem` | `0x14353b8` | `0x146afb8` | 8 | 2/8, 2/8 |

`--class Work` matches 220 tables; **98** of their class names contain `WorkItem`,
including the Windows counterpart of the Linux anchor's item class. `CWorkThreadPool` has three
virtual slots (destructor pair and one 218-byte method), so `AddWorkItem` is not
virtual, exactly as on Linux.

**`AddWorkItem` is a header method.** Its assertion string
(`CWorkItem::SetOrderingKey must be called before AddWorkItem`, from
`common\workthreadpool.h`, public RVA `0x11f54d8`) is referenced **99 times in 87
distinct `.pdata` functions** (public) and **106 times in 93** (publicbeta). It is
inlined into every caller, so no function has it as its own body start. What the
inlined form always ends in is one of two out-of-line routines:

| routine | public | publicbeta | evidence |
| --- | --- | --- | --- |
| job-attached queue add | `0xd39b00` | `0xd657e0` | first call after the assert in 81/87 (87/93 beta) sites; asserts `this == g_pJobCur` (`common\job.cpp`, public RVA `0x142c438`), takes the job in `rcx` and the item in `rdx` |
| plain pool queue add | `0xdbac60` | `0xde9830` | `common\workthreadpool.cpp` method (`0x1459300`); 10 inlined sites call it; asserts `CWorkThreadPool can't support item auto resubmission when ensuring output ordering` and `pWorkThread != NULL`, and reaches the `CWorkThreadPool - Signal Thread Event` path |

Both are stable across builds: the 19 public / 18 beta functions that reference
`workthreadpool.cpp` line up in the same order, and ten of them sit at exactly
a `0x2ebd0` displacement (public `0xdbac60` = beta `0xde9830`); the rest drift by
`0x30`–`0x25a` bytes where code changed, and one is a template instantiation that
moved elsewhere in the image.

The item ABI is *partly* derivable from the tables. In
`CWebSocketConnectionDelayedCloseConnectionWorkItem`, slots 2 and 3 return the
static strings `CWebSocketConnectionDelayedCloseConnectionWorkItem` and
`…::ThreadProcess` (public `0xd6a950`/`0xd6a940`, `lea rax,[rip+…]; ret`), i.e.
the name and thread-process-name getters. The Linux table has 9 slots and MSVC
merges the destructor pair, so Linux slot *n* is Windows slot *n−1* for *n* ≥ 2;
the two getters confirm that mapping (Linux 3/4 → Windows 2/3) and put `execute`
(Linux slot 6) at **Windows slot 5**, which is the item class's only large work
function (251 B, 9 calls, `CTCPConnection` callees). Slots 1/4/7 are one
`return true` stub, 6 is a shared three-store base method, and the base table's
2/3/4 are a shared 26-byte throw stub (pure virtuals).

## What was searched and found nothing

* Neither win64 corpus names a work, job or pool symbol:
  `grep -inE "work|threadpool|job" res/patterns/win64/steamclient/*.toml
  res/patterns/win64/steamui/*.toml` returns only the stats `CAPIJob…` pair.
* `--class ThreadPool` matches exactly one table, `CWorkThreadPool`
  (`CWorkThreadPoolSignal` has no vtable); there is no `CNetThreadPool` or
  per-subsystem pool class, so "the pool Steam's network work runs on" is not
  identifiable by RTTI.
* No string names the post method itself: `strings` finds `AddWorkItem` only
  inside the assertion message, and the Windows binary does not export the
  mangled symbol (unlike the Linux `.so`, where the corpus pattern resolves a
  body).

## What a queue would still need — and why the capability was retired

`CWorkItem`'s `execute` slot is an **inference** from two getters and the Itanium
mapping, never an observation of the pool calling slot 5. A hand-built item that
Steam's network thread dequeues must get that slot right or it crashes the thread
that delivers every inbound packet. On top of that, this host has no anchor for
the pool object (Linux derives `pool_slot` from the post site's own load), no
captured dispatch context (`recv_pkt` has the receiver but never publishes it, and
`Layout` exposes only the decoded `data`/`size` slots, not `conn_id`), and no
published `Alloc`/`Init`/`Release` addresses.

So a Windows queue today would be exactly what a gate is supposed to prevent:
unverified code running on Steam's networking thread. The capability is retired
with that evidence instead. On Windows the response is fabricated and swapped
**inside `CCMInterface::RecvPkt`** — Steam's own call — so there is no cross-thread
hand-off to make and a response that is not ready is not fabricated at all.
`crates/host-windows/src/install/deps.rs` maps `native-response-delivery` onto
`CCMInterface::RecvPkt`, the symbol that serves, and
`crates/host-windows/src/hooks/netpacket/mod.rs` lists what would have to change
for async delivery to be worth building: an after-the-fact producer, a published
dispatch context, published packet constructors, and a post site with the pool
object and the `execute` slot **confirmed by observation** rather than inferred.

**Verdict: no corpus entry. The post machinery is real and the item class is
stable across builds, but the half the host would execute is unverified, and the
synchronous serve path already delivers every response the host can fabricate.**

# Eighth pass: the family lock is client-side, and Steam's own consumers call one query

Section 11 searched for Linux's `CFamilySharing::GetLockingAccount` — a class, a
slot, an `AppId -> account` shape — and found neither, because Windows does not
ship that subsystem. It ships **family groups** instead, and the lock verdict is
made *in the client*: `CUser` keeps a per-app record of who holds a shared app,
recomputes it from the notifications Steam sends, and answers it through one
plain function that both of Steam's own lock consumers call. That function is what
`FamilyLockBypass` was missing, and it is now anchored in both builds.

Section 11's measurements are not retracted: there is still no `CFamilySharing`
class, the 68 `Family`-named RTTI tables are still all 19-slot PBO messages, and
`GetLockingAccount` still occurs zero times. What section 11 did not do is follow
the *strings* out of the message classes into `CUser`. Its own evidence list
(`m_mapFamilyRunningApps`, `CUser::UpdateFamilyRunningApps`, `borrower_id`) named
the carrier; this pass reads it.

## What was measured

Both live builds, engine and tool:

```
vapor-forge-vtable-pe --dll <steamclient64.dll> --module steamclient --class ''     # every RTTI class
vapor-forge-vtable-pe --dll <steamclient64.dll> --module steamclient --class CUserAppManager --all-slots
vapor-forge-pe-probe  --dll <steamclient64.dll> --module steamclient --dump <rva>
vapor-forge-pe-probe  --dll <steamclient64.dll> --module steamclient --callers <rva>
vapor-forge-pe-probe  --dll <steamclient64.dll> --module steamclient --near <rva> --window <hex>
vapor-forge-pattern-mine --verify <corpus.toml>
```

The RTTI names below were enumerated by scanning `.rdata` for `.?AV…` type
descriptors (8 580 in public, 8 682 in publicbeta) rather than by `--class`, which
only reaches classes that own a vtable: a class with no virtual function has a type
descriptor and no table, and the job classes are exactly that case.

## 1. The family-groups classes are messages; the handler is not a class

`--class` with an empty needle lists every RTTI table: 6 523 in public, 6 587 in
publicbeta. Names containing `Family`: **68 in each build, the same 68**, all of
them `CFamilyGroups_*` / `CFamilyGroupsClient_*` protobuf messages or value types.
There is no `CFamilyGroupsClient` **class**: the client half of family groups is
not an object with a vtable, it is `CUser` methods plus protobuf handlers. The
message classes are real (`CFamilyGroupsClient_NotifyRunningApps_Notification` with
its `_PlayingMember` and `_RunningApp` sub-messages), but a message class body is a
serializer, which is why ranking their slots for "takes an AppId, returns an
account" came out empty in section 11 — correctly.

Three RTTI names outside the `Family` set matter, and none has a useful vtable
either:

| class | public | publicbeta | what it is |
| --- | --- | --- | --- |
| `CClientJobSetPreferredLender` | present | present | job wrapper for `CFamilyGroups_SetPreferredLender` |
| `CClientJobSharedLibraryStopPlaying` | present | present | job wrapper for the stop-playing message |
| `CMsgClientSharedLibraryLockStatus` (+`_LockedLibrary`) | present | present | the GC connection's "your library is locked" message |

## 2. Where the client keeps the lock: `m_mapFamilyRunningApps`

The client's own debug serializer names the member and its type in the same call, so
the pair is ground truth rather than inference:

```
lea r8,[rsi+0x6a18]                          ; the member, public
lea r9,[rip+…]      -> "m_mapFamilyRunningApps"
lea rdx,[rip+…]     -> ".?AV?$CUtlMap@IVCSharedRunningApp@@HVCDefLess@@@@"
call  DumpMap(writer, type, member, name)
```

| | public | publicbeta |
| --- | --- | --- |
| serializer function | `0xa23eb0`..`0xa254cc` (5 660 B) | `0xa4d420`..`0xa4ea7c` (5 724 B) |
| member displacement | `+0x6a18` | `+0x6b98` |
| type string | `0x17cd010` | `0x1808dd0` |

`CUtlMap<unsigned int, CSharedRunningApp*, int, CDefLess>` — keyed by `AppId_t`,
in both builds, byte-identical type descriptor. The value is a pointer to a
`CSharedRunningApp`, so the per-app record is a heap object, not an inline struct.

The writer, `CUser::UpdateFamilyRunningApps`, is anchored twice over — by its own
name string and by the `FamilyGroupRunningApp_t` field names it formats for the log:

| | public | publicbeta |
| --- | --- | --- |
| `.pdata` entry | `0xa21d27`..`0xa22481` (1 882 B) | `0xa4b147`..`0xa4b8a1` (1 882 B) |
| `"CUser::UpdateFamilyRunningApps"` xref | `0xa21d63` | `0xa4b183` |
| `"FamilyGroupRunningApp_t"` xref | `0xa220c0` | `0xa4b4eb`, `0xa4b742` |
| field-name strings | `m_unAppID`, `m_bLocked`, `m_unNumMembersPlaying` | same three |

**Identical body size in both builds** — 1 882 bytes — is the strongest
cross-build datum in this pass: the family-running-apps handler has not changed
between the public and publicbeta builds.

It also carries the four strings that name the state machine it drives, all inside
that one function (0 references anywhere else in either module):

```
Shared lock for app %u removed : preferred owner %u
Shared lock for app %u changed : available owner %u, preferred owner %u, no players
Shared lock for app %u changed : available owner %u, preferred owner %u, current player %u
Shared lock for app %u changed : available owner %u, preferred owner %u, %u current players
```

"available owner", "preferred owner" and "current players" are the family-groups
lending model, not the legacy wallet model. The record the function formats for
those lines is `FamilyGroupRunningApp_t { AppId_t m_unAppID; bool m_bLocked;
uint m_unNumMembersPlaying; }` — measured from the formatter's own stores
(`mov [rbp-0x69], ebx`, `mov [rbp-0x65], al`, `mov [rbp-0x61], esi`), not from a
header.

## 3. The query: `CUser::GetFamilySharedLockAccount`

One plain function, 225 bytes in **both** builds, reads that state for one app and
returns an account id:

| | public | publicbeta |
| --- | --- | --- |
| entry | `0xa07460` | `0xa2f7f0` |
| extent | `0xa07460`..`0xa07541` | `0xa2f7f0`..`0xa2f8d1` |
| size | 225 B | 225 B |
| callers | `CUserAppManager` slot 71 `0x4f2300`, launch `0x4ff2c0` | `0x507ff0`, `0x515080` |

Body, identical in both builds apart from two member displacements:

```
48 89 5C 24 08        mov  [rsp+0x8], rbx
48 89 7C 24 10        mov  [rsp+0x10], rdi
44 8B 89 30 6A 00 00  mov  r9d, [rcx+0x6a30]   ; tree root (publicbeta: 0x6bb0)
44 8B DA              mov  r11d, edx           ; the app id
48 8B F9              mov  rdi, rcx
41 83 F9 FF           cmp  r9d, -1
74 54                 je   …                   ; empty tree
48 8B 99 48 6A 00 00  mov  rbx, [rcx+0x6a48]   ; node array (publicbeta: 0x6bc8)
…                     imul r8, rax, 0x38 / cmp r11d, [r8+rbx+0x10]   ; key = the app id
…                     lea  rax, [rcx+0x6a18]   ; the empty-node sentinel
```

The `msflow` shape, all of it measured from the body and the anchor:

* **argument 1** `rcx` — a 64-bit pointer, loaded through `rex.W` opcodes twice.
* **argument 2** `edx` — a **32-bit app id**: `mov r11d, edx`, then compared
  unsigned-less/unsigned-greater against the tree node's 32-bit key at `[node+0x10]`.
  That is the app id, not an account id: both call sites pass the same 32-bit value
  they pass to the ownership-info call.
* **return** `eax` — a 32-bit account id: `xor eax, eax` on the not-found path,
  `mov eax, [r10+rdx*8]` on the found path, then
  `cmp eax, [rdi+0x20a]; cmove eax, ecx` — **the self filter**. `[rcx+0x20a]` is
  the `CSteamID` member `CUser::GetSteamID` returns (`48 8B 81 0A 02 00 00`, the
  leaf anchored in section 10), so "the account is me" is answered `0`.
* **no other arguments**: `r8` and `r9` are never spilled in the prologue and `r9d`
  is overwritten by the first load, so neither is a parameter.
* **no out-parameter**: nothing is written through any pointer, so a replacement
  body can change exactly one thing — the returned account id.

What it does with the app id, decoded: a red-black tree descent on the app id
(`imul …, 0x38` node stride; `<` walks `[node]`, `>` walks `[node+4]`, `==`
terminates), then a 16-byte-stride linear walk of a vector inside the
node's value, comparing one of its fields to a value read from the node, returning
another. The returned account is then compared against our own `CSteamID` and
zeroed when it matches. Read plainly: **"which account, other than me, holds this
app"**, `0` meaning nobody.

One thing this pass could not resolve, and does not need to: the node stride
(0x38) and the inline 0x20-byte value the linear walk reads do not match a
`CSharedRunningApp*` map value (8 bytes), so the tree the query walks is either a
second container whose head sits at the same member base or a `CSharedRunningApp`
layout the formatter does not print. That ambiguity changes no part of the anchor —
the anchor is the function entry — but it is why this section names the container
from the instruction evidence ("a red-black tree") rather than claiming the C++
class.

## 4. Both of Steam's own consumers, in both builds

The query has exactly two callers in each image, and both convert a non-zero answer
into a refusal:

**`CUserAppManager` vtable slot 71** — the play-info fill. (This class *is* in the
corpus: `CUser`'s app manager, 116 slots, class name on the table in both builds.)

```
mov  rcx, [r14+0xd8]            ; the CUser
cmp  [rcx+0x6a10], rsi          ; family group id == 0 -> skip (publicbeta: 0x6b90)
je   skip
mov  edx, r15d
call 0xa07460                   ; GetFamilySharedLockAccount(user, appid)
test eax, eax
je   skip
mov  rcx, [r14+0xd8]
mov  edx, r15d
call 0x9fc910                   ; a package-based "do I have it anyway" check
test al, al
jne  skip
or   [rdi+0x4], 0x100           ; <- the lock flag on the play-info struct
```

| | public | publicbeta |
| --- | --- | --- |
| table | `0x12bbef0`, 116 slots, `class(CUserAppManager)` | `0x12ef5e0`, 116 slots, `class(CUserAppManager)` |
| slot 71 | `0x4f2300` | `0x507ff0` |
| group-id guard | `[rcx+0x6a10]` | `[rcx+0x6b90]` |
| lock flag | `or [rdi+4], 0x100` @ `0x4f243b` | `or [rdi+4], 0x100` @ `0x50812b` |

**The launch path.** `0x4ff2c0` (public) / `0x515080` (publicbeta) walk a chain of
guard checks and return a small result code; when the query reports a holder they
answer `0x21` instead of proceeding:

```
call 0xa07460                   ; public        (publicbeta: call 0xa2f7f0)
test eax, eax
je   next_check
mov  eax, 0x21                  ; public        (publicbeta: identical, 0x51524a)
jmp  epilogue                   ; 0x500983 / 0x516765 -- a plain ret with eax
```

The caller of that path (`0x4eca00` public) stores the code at `[CUser+0x1a0]` and
posts it, so the code is what the launch flow and the UI end up seeing. `0x21` is
therefore the launch result a family-locked app produces.

## 5. How the record is recomputed — why the lock is *client*-side

`UpdateFamilyRunningApps` does not decide on its own; it calls a 625-byte helper
(public `0xa217d0`, `0xa217d0`..`0xa21a41`) that fills the record from two inputs:

1. the app's ownership info — the same call the play-info fill uses
   (`call 0xa03a70`), whose lender field becomes the record's owner;
2. `CUser+0x2480`, a map of **preferred lenders**, consulted only when the
   ownership info reports at least two lenders (`cmp [rbp-0x34], 2; jb …`,
   `call 0xa06e40`, node stride 0x40). Its assert string is
   `pPreferredOwnerDLC` (`0x13a62c8`; publicbeta `0x13db818`).

So the *lender set* arrives from the server (`CFamilyGroups_GetPreferredLenders`,
`CClientJobSetPreferredLender`), but the **verdict** — "is this app playable right
now, and if not who holds it" — is computed and stored locally, and read locally by
one function. A client hook can therefore change the verdict without touching a
server message, which is exactly what section 11 assumed was impossible.

## 6. Corpus entries added and verified

One entry, in both `steamclient` corpora, wildcard-free, uniqueness-checked and
re-verified through the anchor gate:

| symbol | public | publicbeta |
| --- | --- | --- |
| `CUser::GetFamilySharedLockAccount` | `0xa07460`, 1 match, verified | `0xa2f7f0`, 1 match, verified |

```
CUser::GetFamilySharedLockAccount            OK               1  0x0000a07460 verified
CUser::GetFamilySharedLockAccount            OK               1  0x0000a2f7f0 verified
```

The two patterns differ only in the two member displacements (`0x6a30`/`0x6a48`
against `0x6bb0`/`0x6bc8`) — wildcarding them is also unique in both images, and the
two exact forms are what the corpus carries, per build, like every other entry.

`vapor-forge-pattern-mine --verify` still exits non-zero on both files for a
reason that predates this pass: `GetPipeClient` resolves to a body the gate calls
`unmapped` (`0x8c50b0` public, `0x8e6f70` publicbeta). That is an existing corpus
defect outside this pass's scope; every other pattern, including this one, reports
`verified`.

## 7. What a live client would have to show to change this answer

The anchor and the shape are measured; the *sufficiency* of hooking this one query
is not, and cannot be from a static image:

* **Would the play button change?** The flag consumer measured here is
  `CUserAppManager` slot 71 — the struct Steam fills for
  `IClientAppManager`'s app-state queries. If the SteamUI half reads its "In use by
  <member>" label from a *different* path (a cached snapshot, a
  `CFamilyGroupsClient_NotifyRunningApps_Notification` handler in `SteamUI.dll`, or
  the launch result it was posted), the label would stay. Live test: enable
  `family-lock-bypass`, have a member start the app, and check whether the button
  flips *and* whether `Shared lock for app … changed` still logs with a non-zero
  available owner — the log line proves the record is still locked while the
  button is enabled, i.e. that the hook, not the record, is what changed.
* **Would the launch succeed?** The launch path answers `0x21` from the same query,
  so the refusal code should disappear. A client that still refuses has a second
  gate; the next place to look is `CUserAppManager` slot 71's `0x9fc910`
  (package-based ownership), which is *after* the query and not bypassed by it.
* **Does anything else call the record directly?** `UpdateFamilyRunningApps`
  recomputes on every notification, so a bypass survives a later
  `NotifyRunningApps` — but a consumer that reads `m_mapFamilyRunningApps` without
  going through this query would keep the lock. The serializer walk
  (`0xa23eb0`) shows the map is also readable; other functions in the `CUser`
  code blob use the same member displacement, and only this query has the
  `AppId -> account` shape with Steam's two lock consumers on the other end.

What would refute the anchor: a live build where the query's return stops being the
account that gates the launch (for example if Valve moved the check into the
`SteamUI.dll` side, which this pass did not measure — `SteamUI.dll` has its own
corpus but no family symbol in it).

**Verdict: anchored, and wired.** Windows decides the family shared-library lock
**client-side**, in `CUser`: `m_mapFamilyRunningApps` holds
`CSharedRunningApp` records, `CUser::UpdateFamilyRunningApps` recomputes them from
`CFamilyGroupsClient_NotifyRunningApps_Notification` and the preferred-lender map,
and `CUser::GetFamilySharedLockAccount(this, appid) -> u32` reads them for both of
Steam's own consumers — the `0x100` play-info lock flag and the `0x21` launch
result. The capability is no longer retired: it is
`crates/host-windows/src/hooks/family_lock.rs`, gated by `family-lock-bypass`, and
mapped in `install/deps.rs` to the symbol that truly backs it.
