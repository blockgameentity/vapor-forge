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

The miner exits non-zero when fewer than half of the hint symbols resolve, so a
drifted build fails the job instead of shipping a half-empty corpus.

## Known gaps

No hint was ever published for `CUser::GetSubscribedApps`, the stats path, the
`CNetPacket` dispatch site or a `SetEnvString` equivalent, so those symbols are
absent rather than missed. They need derivation from the binary itself (phase 0
follow-up), not from the hint corpus.
