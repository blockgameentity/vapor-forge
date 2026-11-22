# Gate 0 behaviour probes — four underived symbols

Four Windows targets were never published in any hint corpus, so mining cannot
reach them: `CUser::GetSubscribedApps`, the user-stats/achievement path, the
`CNetPacket` dispatch site, and a `SetEnvString` equivalent. This report records
what `vapor-forge-pe-probe` measured on the two live builds and whether any of
the four can be shortlisted to a credible candidate.

**Bottom line: none of the four is shortlisted.** The tool ranks every function
in the module by behavioural predicates and prints the numbers behind each rank;
every top candidate below was *read* (`--dump`) and refuted. What each one is
instead is stated, together with the evidence that would confirm the real symbol.
The negatives are the deliverable: they say where not to look.

Builds measured (both in `/tmp/gate0/`):

| channel | build | `steamclient64.dll` sha256 | `.pdata` entries | merged extents | measured (>= 0x18 B) |
| --- | --- | --- | --- | --- | --- |
| public | 1794536281 | `669fa08e259277fb…` | 92,949 | 49,230 | 49,087 |
| publicbeta | 1795048523 | `4a2d79520a72c558…` | 93,858 | 49,694 | 49,550 |

92,949 raw `RUNTIME_FUNCTION` entries merge to 49,230 extents because contiguous
fragments (prolog, body, funclets) are joined exactly as
`vapor-forge-pe-functions` joins them. Two consequences for reading the output:
extents are upper bounds, and 653 merged extents begin at a non-16-aligned RVA
(e.g. the userstats candidate `0xa2c0d`), which means the extent is a merge chain
rather than one clean function.

> **Superseded 2026-11-23.** The rule and the counts above are replaced by the
> single documented `.pdata` definition in
> [`README.md`](./README.md#function-identity-the-pdata-rule): the same table now
> merges to 52,946 functions for this binary (aligned splits of one function are
> joined, aligned flush entries after a terminating extent are not), and 49,087
> is the probe's own `< 0x18` sweep filter, not a function count. A non-16-aligned
> extent is a byte-packed thunk, not evidence of a merge chain. The probe's
> behaviour measurements are unaffected; only the function identity behind them
> changed.

## What is measured

Per function, from one linear decode of the `.pdata` extent:

* MS-x64 argument shape (`crates/patterns/src/msabi/`): for each of
  `rcx/rdx/r8/r9`, the first read, the first write, and whether it is read before
  being written; the first use is classified as operand, memory address,
  forwarding copy, home-slot spill, or plain overwrite. This is what separates a
  four-argument writer from a three-argument sibling with the same prologue.
* 32-bit loads and stores whose address base traces back to an argument pointer
  (`mov [rdx+rax*4], ecx` counts as a store through argument 1), including
  pointers reloaded from the home slots after a spill. Provenance is dropped at
  any unconditional branch and at every call for the volatile registers.
* Loop count (distinct backward branch targets) and how many argument-pointer
  stores sit inside a backward-branch span.
* String-build evidence: 1-byte immediate stores, a stored literal `'='`
  (`0x3d`), and all-printable immediate chunks.
* Dispatch evidence: per-register compare chains, indexed table loads, memory
  jump tables, and register indirect branches (MSVC reaches a relative-offset
  table with `movsxd rax,[t+idx*4]` / `add rax,t` / `jmp rax`, so a memory-only
  jump-table test would have found nothing: it found zero in 3,197 candidates).
* Direct callees as RVAs, nameable through the mined corpus; `--callers` prints
  who calls a candidate.
* Shape-plus-behaviour fingerprints of a second build, so a candidate that
  reappears exactly once there is visible as such.

Reproduce:

```sh
cargo build -p vapor-forge-patterns --features tools --bin vapor-forge-pe-probe
PUB=res/patterns/win64/steamclient/669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b.toml
BETA=res/patterns/win64/steamclient/4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10.toml
./target/debug/vapor-forge-pe-probe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --toml $PUB --against /tmp/gate0/publicbeta/steamclient64.dll --top 8
./target/debug/vapor-forge-pe-probe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --dump <rva>        # read one candidate
./target/debug/vapor-forge-pe-probe --dll /tmp/gate0/public/steamclient64.dll \
  --module steamclient --toml $PUB --callers <rva>
```

All 14 public anchors and all 13 beta anchors resolve. Of the 16 candidates in
each final dump, 11 (public) and 10 (beta) have an exact second-build counterpart
— the shapes are stable, they are just the wrong shapes.

## 1. `CUser::GetSubscribedApps` — OPEN

Profile: `rcx` (this) + `rdx` read as a pointer + `r8` (max) + `r9` (bool);
one u32 store site through argument 2 repeated in one loop; no callees, no
multiply/divide, nothing read from the stack. 204 (public) / 200 (beta)
functions pass the gate.

| build | rva | size | insns | ± | evidence | refuted by reading |
| --- | --- | --- | --- | --- | --- | --- |
| public | `0xbed380` | 0xa4 | 44 | 26 | arity 4, 1 u32 store via rdx, 1 loop, 1 loop store, 0 calls, **3 multiplies**; xbuild exact `0xc17a70` | `imul`/`sar 0x10` gain scaling — a resampler, not a copy loop |
| beta | `0xc17a70` | 0xa4 | 44 | 26 | identical to the row above | same |
| public | `0xfdbd1c` | 0x12b | 85 | 26 | arity 4, 4 u32 stores via rdx, 0 calls, 1 stack-arg read; xbuild exact `0x100b63c` | bit-flag deserialiser: `test bl,4/8/10`, table-driven shift extraction |
| beta | `0x100b63c` | 0x12b | 85 | 26 | identical | same |
| public | `0x1054db0` | 0x5e4 | 353 | 26 | arity 3, 48 table loads, 10 byte-arg reads; xbuild exact `0x10846c0` | base64 alphabet lookup (`[rcx+rax*1+…]`, `and 0x3f`) |
| beta | `0x10846c0` | 0x5e4 | 353 | 26 | identical | same |

The Linux sibling is known, and its evidence is far more specific than any
profile I could justify blind: `validate_get_subscribed_apps32` requires
`movzx eax, byte ptr [ebp+0x14]` (the `bool` argument consumed as a byte) **and**
a nested `lea` chain `i*8` → `base + i*8` over entries of `9*8 = 0x48` bytes, plus
a field group `0xc` apart. Encoding those two as Windows facts (`stride9_leas`,
`byte_argument_reads`) and intersecting with "u32 store through argument 2"
leaves **6 of 204** candidates, and reading all six refutes them:

| rva | what it is on reading |
| --- | --- |
| `0x6b73f0` | floating-point scaling of `this` fields (`mulsd xmm0, [rip+…]`, `[rcx+0x3c0]`) |
| `0x574db0` | argument validation: `cmp [rdx+8], 0xfffffe`, then an assert call with two string literals |
| `0x1010488`, `0x1010580` | stream reader over a `{base,cur,count,limit}` buffer (`[rbx+0x0]`, `[rbx+0x10]`, `[rbx+0x20]`) |
| `0xc38f90`, `0xc38310` | shared-pointer refcount walk (`test r9b, 3`, `shr eax,1`, `or [rdx+0x10], 1`) |

**Confirm:** `movzx rX, r9b`, an `i*9` stride (`lea r,[r+r*8]` or `imul r,9`)
and a u32 store through `rdx` inside the *same* loop, plus a caller that passes a
stack `AppId_t[]` and a count (search with `--callers`).
**Refute:** any `imul`/`sar` scaling pair, any byte-flag branch tree, or a read
back through the out pointer.

## 2. User-stats / achievement path — OPEN, profile unusable

Profile: at least `(this, appid)` plus u32 memory traffic through `this`, 2..64
u32 stores, >= 2 calls, a loop, no `'='` immediates. Gate passes for **16,492
(public) / 16,620 (beta)** functions and the top score is 9 of a possible 9 — the
profile saturates, so the ranking carries no information. Top entries
(`0xa2c0d`, `0xa388d`, 11 u32 stores, 3 calls, both present at the same RVA in
both builds) are not distinguishable from thousands of others, and `0xa2c0d` is
one of the unaligned merge chains noted above.

This is a design failure, not a coincidence: "reads and writes a struct through
`this` and calls something" describes most of the module. A stats path needs a
*different* anchor — the `IClientUserStats` vtable slot it calls — not a looser
shape.

**Confirm:** a function whose direct callee set includes an anchored
`IClientUserStats`-style method (extend the corpus) or which is reached from a
`GetUserStats(appid, …)` shaped caller (`--callers`), and whose `this` fields are
touched at a fixed stride.
**Refute:** string building, base64/bit-packing, or a handler-table indirect
call.

## 3. `CNetPacket` dispatch site — OPEN

Profile: a table-driven branch (indexed memory branch, or the MSVC
relative-offset idiom of a table load plus a register branch), or a compare chain
of >= 8 values on one register with a maximum <= 0x4000. 3,197 (public) / 3,226
(beta) functions qualify.

| build | rva | size | insns | chain | max imm | loops | calls | refuted by reading |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| public | `0x3a45f0` | 0x228c | 2041 | 56 | 0xf8 | 20 | 96 | protobuf wire parser: `cmp esi,0x80` varint continuation, `shr eax,3` field number, `jmp rcx` over a relative table |
| beta | `0x3afc20` | 0x228c | 2046 | 56 | 0xf8 | 21 | 96 | same function, same shape |
| public | `0x10b580` | 0x738 | 487 | 19 | 0x9a | 9 | 20 | sibling protobuf parser |
| public | `0x110130` | 0x768 | 517 | 17 | 0x88 | 6 | 18 | sibling protobuf parser |

Across the top 300 candidates the longest single-register chain is 56 and no
candidate with a chain >= 32 is straight-line (all of them loop). Compare chains
in this module top out at `0xfa`, which is the protobuf field-tag ceiling: every
long chain found is a field-number dispatch. A `CNetPacket` EMsg dispatch is not
a compare chain here.

The `--near` search around the anchored EMsg code does not rescue it: within
±0x8000 of `PchMsgNameFromEMsg` (`0xd4c460`) the largest candidate,
`0xd474a0` (0x1892 bytes, 1447 insns, 134 calls, 31 table loads), turns out on
reading to be a static-initialiser/teardown that nulls 17 global slots.

**Confirm:** the packet's id field is read once (`mov eax, [pkt+off]`) and then
either a straight-line chain or a table over **>= 100** EMsg constants selects a
handler, with one call per case; a caller that has just filled a `CNetPacket`
(`CNetPacket::Init`-style store set) is the strongest corroboration.
**Refute:** a loop around the compares, or `shr eax, 3` / `and eax, 7` before the
compare (that is a wire format, not a message id).

## 4. `SetEnvString` equivalent — OPEN

Profile: string-build evidence (a stored literal `'='` and/or all-printable
immediates), >= 3 arguments, a store through an argument pointer inside a loop.
307 (public) / 298 (beta) functions qualify.

| build | rva | size | insns | `'='`-imm | printable-imm | byte-arg reads | calls | refuted by reading |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| public | `0xdb9bc0` | 0x3cb | 265 | 1 | 3 | 9 | 23 | base64 encoder: the `'='` is padding (`and al, 0x3f`, alphabet table) |
| beta | `0xde8780` | 0x3cb | 265 | 1 | 3 | 9 | 23 | same function |
| public | `0x3265a0` | 0x58e | 408 | 1 | 3 | 15 | 19 | same base64/serialiser family |
| public | `0x1053f30` | 0x347 | 231 | 2 | 2 | 16 | 5 | base64 encoder: `sar al,2`, `and al,0x3f`, append into a `std::string` at `[rdi+0x10]` |
| public | `0xbab640` | 0x3e8 | 281 | 1* | 2 | 3 | 23 | KeyValues text parser: `cmp [rdi],0x3d` and `cmp sil,0x3a` are compares, `cmp [rdi],0xa` counts lines |

`*` measured before a fix described below.

The environment-block writer would append into a caller-supplied buffer, so its
`'='` is a **store**, not a compare, and it needs no more than a copy loop behind
it. Nothing in the top 300 has that combination: candidates with `'='` stores are
base64 padding, VDF writers, or serialisers with 15–50 callees.

**Confirm:** 3–5 arguments, a 1-byte store of `0x3d` between two copies of
caller-supplied strings, a buffer pointer that is written and never read back,
and a caller that also writes `NAME=steam` style entries (spawn block).
**Refute:** `'='` appearing only in `cmp`, or a multi-byte immediate that merely
contains the byte.

## Tool defects found and fixed while measuring

Reporting these because the numbers above depend on the fixes:

1. `cmp [reg], 0x3d` was counted as a *store* of `'='`. Fixing it moved the
   envstring gate from 680 to 307 functions and removed the KeyValues-parser
   false positive at the top.
2. A wide immediate that merely contains byte `0x3d` (e.g. the magic
   `0xef103d19` in `0x797ea0`) counted as a string build. Wide immediates now
   only count when every byte is printable.
3. The first jump-table test (indexed *memory* indirect branch) matched zero
   functions in 49,087. MSVC reaches relative-offset tables with
   `movsxd rax,[t+idx*4]` + `jmp rax`, so table loads and register branches are
   now measured separately.
4. Compare chains were counted as one global set. They are now per register: the
   dispatch question is "how many constants is *this* value compared against",
   not how many compares the function contains.

## What would actually move these four

* The machine-readable EMsg constant list (or `PchMsgNameFromEMsg`'s own jump
  table) would turn the dispatch profile from "table-driven switch" into
  "switch over these 500 values", which is decisive. The anchored name function
  is the place to extract it from; that is a separate slice.
* `CUser::GetSubscribedApps` needs the *call site* rather than the body: a caller
  that allocates `max * 4` bytes and passes it with a count pins the symbol even
  if the body is inlined or refactored. `--callers` is already the tool for that;
  what is missing is an anchor on the caller.
* The user-stats path needs a corpus anchor on an `IClientUserStats` method. No
  behavioural profile of the kind implemented here can isolate it: 16,492
  functions match, and the profile's own top score is the maximum, which is the
  definition of an uninformative ranking.
* All four would benefit from one more fact: *which* field offset of an
  argument's object is read first. A message dispatch reads the id field of the
  packet; `GetSubscribedApps` reads the vector base and count of `this`. That
  fact is not implemented yet and is the obvious next predicate.
