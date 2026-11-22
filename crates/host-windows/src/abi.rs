//! The Windows call shapes the hook bodies stand in for.
//!
//! Every type in this module separates two kinds of fact, and the separation is
//! load bearing: treat an assumption as measured and a hook corrupts a live
//! client.
//!
//! * **Verified** — decoded from the corpus pattern that anchors the symbol
//!   (`res/patterns/win64/<component>/<sha256>.toml`). An MS-x64 function spills
//!   its register arguments to their home slots, so the *store width* in the
//!   anchoring pattern measures how wide that argument is: `89 50 10`
//!   (`mov [rax+0x10], edx`) proves the second integer argument — `rdx` — is 32
//!   bits, and `4C 89 48 20` proves the fourth — `r9` — is 64. Nothing else is
//!   measured. The corpus carries no argument counts, no `.pdata` layout facts,
//!   and no Windows build has ever executed this host.
//! * **Assumed** — taken from the Linux host's measured ABI
//!   (`crates/steam-native-abi`, which derives layouts from
//!   `steamclientpublic.h`) or from the symbol's own name. Each type says which
//!   of its fields fall in which class.
//!
//! Two consequences hold for every body in [`crate::hooks`]:
//!
//! 1. A register slot that a body does not need is still *forwarded verbatim*.
//!    A Rust replacement function clobbers the volatile registers, so a dropped
//!    slot is a lost argument, not a harmless omission.
//! 2. A return value is read as a byte, never as a Rust `bool`. Steam returns
//!    `bool` in `al`; constructing a `bool` from a byte we did not produce would
//!    be undefined behaviour, so the trampoline types return `u8` and every body
//!    produces its own `0`/`1`.
//!
//! One module here documents a shape that is read by an instruction, not by a
//! store: [`achievements`] records the two anchored `CAPIJobRequestUserStats`
//! sites, the `EResult` dispatch, and — the reason it exists — the fact that the
//! job object's fields are measured rather than named, so the crate no longer
//! carries a per-build offset table. Its merge type has a body
//! ([`crate::hooks::achievements`]), which rewrites the reply body when the corpus
//! decoder resolves where that body lives.

pub mod achievements;
pub mod callbacks;
pub mod cloud;
pub mod depot;
pub mod dlc;
pub mod family_lock;
pub mod launch_env;
pub mod legacy_cdkey;
pub mod netpacket;
pub mod ownership;
pub mod package;
pub mod shader;
pub mod steam_id;
pub mod subscribed;
pub mod ticket;
pub mod ui;

/// The one value a replacement returns for a Steam function that returns `bool`.
///
/// Our own decisions are real `bool`s, so this cannot inherit a foreign bit
/// pattern; the trampoline type stays `u8` so a call *through* the original never
/// materialises an invalid `bool`.
pub(crate) fn ret(value: bool) -> u8 {
    u8::from(value)
}
