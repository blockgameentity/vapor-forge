//! `CCMInterface::RecvPkt` — the CM message dispatch the request-code body stands in for.
//!
//! # Verified from the corpus
//!
//! The symbol is anchored in both live builds
//! (`res/patterns/win64/steamclient/669fa08e….toml` public and `4a2d7952….toml`
//! publicbeta), so [`crate::hooks::netpacket`] can install:
//!
//! | build | RVA | source |
//! | --- | --- | --- |
//! | public | `0x5d3030` | pattern `48 8B C4 55 48 8D A8 E8 FB FF FF 48 81 EC 10 05 00 00 …` |
//! | publicbeta | `0x5f0b50` | same pattern, same first 22 bytes |
//!
//! `res/patterns/win64/VTABLE-REPORT.md` §3 identifies it as `CCMInterface`
//! vtable slot 3 (reached from the 167-byte `CCMConnection::RecvPkt` forwarder)
//! and refutes the name `CNetPacket::Dispatch`: the packet classes have no
//! dispatch slot at all.
//!
//! The call shape below is measured from both images, not guessed. The first
//! 0x2a bytes are byte-identical in public (`0x1385d3030`) and publicbeta
//! (`0x1385f0b50`) — they diverge only in the `%rip` displacement of the first
//! global read, well past the argument spills:
//!
//! ```text
//! mov  %rsp,%rax ; push %rbp ; lea -0x418(%rax),%rbp ; sub $0x510,%rsp
//! mov  %rbx,0x8(%rax)      ; spill
//! mov  %rdx,%rbx           ; rbx = arg2  -> the packet
//! mov  %rsi,-0x10(%rax)    ; spill
//! mov  %rcx,%rsi           ; rsi = arg1  -> this (CCMInterface)
//! ```
//!
//! So: two arguments, `rcx` = receiver, `rdx` = the packet, no message id in a
//! register. The function returns through one shared epilogue
//! (`add $0x510,%rsp ; pop %rbp ; ret`, public `0x1385d4034`) that sets no return
//! register, i.e. the return type is `void`, exactly like the Linux host's
//! `hk_recv_pkt(this, packet)`.
//!
//! # Assumed
//!
//! * That `rdx` is a `CNetPacket *` and not some other context object. Steam
//!   passes it to the EMsg accessor (below) and every handler consumes it, and
//!   `CCMConnection::RecvPkt` forwards its own packet argument straight here, but
//!   nothing in the corpus measures the object's layout. [`RecvPktFn`] therefore
//!   types it as an opaque pointer: the body never dereferences it.
//! * That the dispatch's work-item/`this` state at `[rsi+0x138]` (read by
//!   `cmp %r13d,0x138(%rsi)` at public `0x1385d30f4`) does not matter for a
//!   pass-through: the body writes no receiver field.
//!
//! # Missing (measured, and the reason this hook is not an interception)
//!
//! Two capabilities the Linux body has and this one does not, both named in
//! [`crate::hooks::netpacket`]:
//!
//! 1. **The packet layout.** `RecvPkt` reads the payload through
//!    `IMsgNetPacket`'s own vtable, not through a struct offset, and the offsets
//!    (`data`, `size`, `owned_data`) are decoded from `CNetPacket::Init` — which
//!    **no win64 corpus file anchors**. `CNetPacket::Init`, `CNetPacket::Alloc`
//!    and `CNetPacket::Release` appear in `res/patterns/x86_64.toml` (the Linux
//!    corpus) and in neither `res/patterns/win64/steamclient/*.toml`. Without
//!    them [`Layout`] cannot be decoded, so no payload byte, no `EMsg` and no
//!    `(depot, gid)` is readable here.
//! 2. **The EMsg accessor.** The dispatch obtains the message id as
//!    `(*0x138d4bec0(packet))->vtable[0x60/8]()` (public; the beta calls the same
//!    shape at `0x138d788c0`). The helper is a deserialising accessor — it reads
//!    packet fields at `+0x10`/`+0x18`/`+0x28`, size-checks them and builds an
//!    object — and no corpus pattern anchors it either. Calling an unanchored RVA
//!    from a hook body would be a pinned address in disguise, which is exactly
//!    what the corpus exists to avoid.
//!
//! Both are reported as a disabled capability rather than skipped silently: see
//! `crate::hooks::netpacket::availability`.

use core::ffi::c_void;

/// `RecvPkt(this, packet) -> void`.
///
/// Both arguments are forwarded to the original untouched; the body treats the
/// packet as opaque, because its layout is unmeasured (module docs, "Missing").
pub type RecvPktFn = unsafe extern "system" fn(this: *mut c_void, packet: *mut c_void);

/// Offsets of the `CNetPacket` fields a response rewrite would have to swap.
///
/// Nothing constructs one of these on Windows today: the offsets are decoded from
/// `CNetPacket::Init`, which the win64 corpus does not anchor (module docs,
/// "Missing"). The type exists because the decision table in
/// [`crate::hooks::netpacket::decision`] is written for the day it lands, and
/// because the validation below is the part that must not be improvised at that
/// moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    /// Offset of the `const uint8 *m_pData` slot.
    pub data: usize,
    /// Offset of the `uint32 m_cubData` slot.
    pub size: usize,
}

impl Layout {
    /// `None` unless both slots are naturally aligned and do not overlap.
    ///
    /// A decoded layout that overlaps would make a rewrite corrupt the packet's
    /// own pointer with its length; refusing it here means the caller falls
    /// through instead.
    pub fn new(data: usize, size: usize) -> Option<Self> {
        let pointer = core::mem::size_of::<usize>();
        let width = core::mem::size_of::<u32>();
        let aligned = data % pointer == 0 && size % width == 0;
        let disjoint = data + pointer <= size || size + width <= data;
        (aligned && disjoint).then_some(Self { data, size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_rejects_overlapping_or_misaligned_slots() {
        assert_eq!(Layout::new(8, 16), Some(Layout { data: 8, size: 16 }));
        // Size inside the pointer slot: a rewrite would corrupt the pointer.
        assert_eq!(Layout::new(8, 12), None);
        // Misaligned pointer slot on any 64-bit target.
        assert_eq!(Layout::new(9, 24), None);
        // Misaligned size slot.
        assert_eq!(Layout::new(8, 19), None);
    }

    #[test]
    fn the_recv_packet_signature_is_two_arguments_and_no_result() {
        // A type-level assertion: `RecvPktFn` returns nothing, so a body cannot
        // invent a return value for a void entry.
        unsafe extern "system" fn shape(_this: *mut c_void, _packet: *mut c_void) {}
        let typed: RecvPktFn = shape;
        assert_eq!(
            typed as usize,
            shape as unsafe extern "system" fn(*mut c_void, *mut c_void) as usize
        );
    }
}
