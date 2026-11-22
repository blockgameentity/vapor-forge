# Windows pattern corpus (Gate 0, public + publicbeta)

Layout is keyed by the **sha256 of the module binary**, not by architecture:

```
win64/steamclient/<sha256 of steamclient64.dll>.toml
win64/steamui/<sha256 of SteamUI.dll>.toml
```

Architecture-keyed files (`res/patterns/x86_64.toml`) cannot serve Windows: two
Steam builds are always live (public + publicbeta) and their encodings differ, so
one file per arch would either need variants that must agree (they do not, they
describe different builds) or would silently pick one build's encoding.

## Function identity: the `.pdata` rule

Every tool in this tree resolves an RVA to a function through one module,
`vapor_forge_patterns::pdata`; its module docs (and `pdata::fold`) carry the
normative rule. Summary:

1. Read every whole `RUNTIME_FUNCTION` (12 bytes, `IMAGE_DIRECTORY_ENTRY_EXCEPTION`).
   A table whose size is not a multiple of 12, or whose mapped range leaves the
   file, is an **error**, never a silently short table.
2. Drop `end <= begin` entries (counted; none occur in these four modules).
3. Sort by `begin`.
4. An entry *joins* the previous extent when it is a fragment of it:
   * `begin < previous.end` — an overlap (table anomaly; the later `end` wins);
   * `begin == previous.end && begin % 16 != 0` — flush but unaligned, where MSVC
     puts funclets and cold fragments;
   * `begin == previous.end && begin % 16 == 0` **and the previous extent has no
     terminator** (no `ret`, no tail jump, after skipping `nop`/`int3` padding) —
     the linker split one function at an aligned boundary.
   A flush **aligned** entry after a *terminating* extent is an independent
   function and starts a new extent.
5. A merged extent is **never** dropped for being short. `< 0x18` is a metric and
   a class (`Body::is_thunk`), not a filter.

Why both halves of rule 4 are needed, measured on public `steamclient64.dll` and
public `SteamUI.dll`:

* Contiguity alone **over**-merges, because the linker also places independent
  functions flush: `0xb2ea0`, `0xb2ee0`, `0xb2f20`, …, `0xb30a0` in
  `steamclient64.dll` are nine consecutive 64-byte functions, each with its own
  prologue and tail jump, and publicbeta `GetAppDataFromAppInfo` at `0x4f37e0`
  (541 bytes, the same size as its public-build extent) sits flush after a
  27-byte fragment entry. Both would be folded into the extent before them and
  then rejected by the gate.
* Alignment alone **under**-merges, because the linker also splits one function
  at an aligned boundary: `LoadModuleWithPath` (`SteamUI.dll`) has a 16-byte
  prologue entry at `0x8dd870` whose body continues in a separate aligned entry
  at `0x8dd880` (774 bytes when joined), and `CUpdateManager::MarkAppChange` ends
  mid-flow and continues in an epilogue entry at `0x6cf650` (721 bytes when
  joined). Those split entries have no terminator, which is exactly what the
  terminator clause tests.
* No entry ever overlaps another, so the overlap clause never fires on these
  images; it exists so the rule is total and loses no byte on a corrupt table.

Measured fragment shapes (all four modules): no overlapping entries, no
`end <= begin` entries, table sizes a whole multiple of 12. The two numbers this
repository used before this rule were not two definitions: 49,230 was the
contiguous-merge count for public `steamclient64.dll` and 49,087 was those 143
extents shorter than `0x18` removed by the probe's sweep filter.

## Function table, anchors, and the anchor gate

`vapor-forge-pe-functions --dll <pe> --toml <corpus> --metrics` and
`vapor-forge-pattern-mine --verify <corpus>` now agree on identity: an RVA is a
**function entry** iff it equals a merged extent's `begin`. The anchor gate
additionally requires the entry's body to contain a `RET` or end in a tail jump
(ignoring `nop`/`int3` padding); a mid-function hit, a fragment, an extent with no
terminator and an RVA outside `.pdata` are all rejected. `vapor-forge-pattern-mine`
applies it by default when mining and when verifying; `--allow-unverified-anchors`
opts out.

Measured 2026-11-23 with build `1794536281` (public) / `1795048523` (publicbeta):

| channel | module | sha256 | raw `.pdata` entries | functions (merged) | short extents < `0x18` | corpus patterns | on a function entry | passing the body gate |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| public | `steamclient64.dll` | `669fa08e2592…` | 92,949 | **52,946** | 254 | 14 | 13 | 13 |
| public | `SteamUI.dll` | `2dfb0a5d50dd…` | 72,223 | **40,826** | 200 | 7 | 7 | 7 |
| publicbeta | `steamclient64.dll` | `4a2d79520a72…` | 93,858 | **53,430** | 253 | 13 | 12 | 12 |
| publicbeta | `SteamUI.dll` | `55f7a4f9ad55…` | 72,694 | **41,159** | 204 | 5 | 5 | 5 |

Per-fragment breakdown for those four tables (`--metrics`):

| channel | module | unaligned fragment joins | aligned continuation joins | aligned flush = own function | unaligned extents | terminating bodies | tail-jump thunks |
| --- | --- | --- | --- | --- | --- | --- | --- |
| public | `steamclient64.dll` | 37,903 | 2,100 | 3,716 | 653 | 52,754 | 395 |
| public | `SteamUI.dll` | 29,749 | 1,648 | 2,672 | 635 | 40,659 | 403 |
| publicbeta | `steamclient64.dll` | 38,293 | 2,135 | 3,736 | 650 | 53,238 | 395 |
| publicbeta | `SteamUI.dll` | 29,851 | 1,684 | 2,702 | 623 | 40,989 | 405 |

### The gate rejects one shipped symbol

`GetPipeClient` (both `steamclient` corpus files, pattern
`85 D2 74 ?? 44 0F B7 CA 44 3B 49 60 73`) resolves uniquely to `0x8c50b0`
(public) / `0x8e6f70` (publicbeta), which is **outside `.pdata`**: the region
`0x8c50b0..0x8c5160` carries no unwind info (it follows a `lea`/`ret` ending at
`0x8c50a8` and `cc` padding), and the matched bytes are a mid-body
`test edx, edx; je …`, not a prologue. So the pattern is not a hook target under
any `.pdata` definition. **The corpus was not regenerated**: 37 of 39 shipped
patterns pass the gate, and that one is left in place for a human decision (fix
the hint, or drop the symbol). Everything else resolves on an entry whose body
returns.

## Provenance (Gate 0, mined 2026-11-22)

| build | channel | steamclient64.dll | SteamUI.dll | mined |
| --- | --- | --- | --- | --- |
| 1794536281 | public | `669fa08e2592…` | `2dfb0a5d50dd…` | 14 + 7 |
| 1795048523 | publicbeta | `4a2d79520a72…` | `55f7a4f9ad55…` | 13 + 5 |

Derived with `vapor-forge-pattern-mine` from the OST `steam-monitor` hint corpus
(44 historical builds, last published 2026-09-06 for build 1788652215 — **no hint
file exists for the current builds**). Hints supplied candidate prologues only;
every shipped pattern was found in the current image and grown until it matched
exactly once, then re-parsed and re-resolved with the shipped scanner
(`scan_code_segment`). No RVA is stored and none is used for resolution.

Rejected on purpose: hints whose prologue is shared by two symbols (a published
prologue is not a signature — `ConfigStoreGetBinary` and `LoadDepotDecryptionKey`
carry identical hint bytes). Those symbols are reported as `COLLIDE` and left out.

## Re-mine after a Steam update

```sh
cargo build -p vapor-forge-patterns --features tools --bin vapor-forge-pattern-mine

./target/debug/vapor-forge-pattern-mine \
  --dll /cache/win64/steamclient64.dll --module steamclient --arch x86_64 \
  $(for f in hints/steamclient/*.toml; do printf -- '--hints %s ' "$f"; done) \
  --out res/patterns/win64/steamclient/<sha256>.toml

./target/debug/vapor-forge-pattern-mine \
  --dll /cache/win64/steamclient64.dll --module steamclient --arch x86_64 \
  --verify res/patterns/win64/steamclient/<sha256>.toml
```

Both mining and `--verify` apply the anchor gate (see above) and report the
verdict per symbol; a symbol that resolves off-entry or into a body without a
terminator fails the run instead of being written out, and
`--allow-unverified-anchors` is the explicit opt-out. Check the table numbers with
`vapor-forge-pe-functions --dll <pe> --metrics` before and after re-mining: a
change in the merged-function count for the same binary means the rule, not the
build, moved.

The miner exits non-zero when fewer than half of the hint symbols resolve, so a
drifted build fails the job instead of shipping a half-empty corpus.

## Known gaps

No hint was ever published for `CUser::GetSubscribedApps`, the stats path, the
`CNetPacket` dispatch site or a `SetEnvString` equivalent, so those symbols are
absent rather than missed. They need derivation from the binary itself (phase 0
follow-up), not from the hint corpus.

Derivation is attempted in [`PROBE-REPORT.md`](./PROBE-REPORT.md), which records
what `vapor-forge-pe-probe` measured on both builds. As of that report none of
the four is shortlisted: every top candidate was read and refuted, and the report
states per symbol what evidence would confirm or refute it. Re-run the report
after adding a corpus anchor that a candidate can be checked against (a caller,
or an `IClientUserStats` method) rather than after adding a looser profile.
