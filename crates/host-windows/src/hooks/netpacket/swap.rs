//! The swap-and-restore discipline for one inbound packet.
//!
//! This is the Windows counterpart of the Linux host's
//! `netpacket::boundary::PacketSwapGuard`: read the packet's `(data, size)` slots
//! through the decoded [`Layout`], keep the replacement bytes alive in a
//! `Box<[u8]>` the guard owns, and write Steam's two values back when the guard is
//! dropped — which the body does immediately after the original `RecvPkt` returns.
//!
//! # Why the guard, and not "write, call, write back"
//!
//! `RecvPkt` dispatches synchronously and Steam's own caller releases the packet,
//! so the replacement bytes must outlive the call and Steam's pointer must be back
//! before it does. A guard makes both properties structural: the bytes are owned
//! by the guard, and [`Drop`] is the only path that ends the swap. There is no
//! early return in the body that can skip it.
//!
//! # What is never done
//!
//! * No allocation happens while the packet is swapped: the replacement is built
//!   (and encoded) before [`PacketSwap::install`] writes anything.
//! * No lock is held: the guard stores raw slots, and the caller's state lookups
//!   are finished before the swap is installed.
//! * Nothing outside the two decoded slots is written, which the tests assert by
//!   filling the rest of a fake packet with a sentinel.

use crate::abi::netpacket::Layout;

/// One installed swap: the replacement bytes, and the two slots to restore.
pub struct PacketSwap {
    data_slot: *mut *const u8,
    size_slot: *mut u32,
    original_data: *const u8,
    original_size: u32,
    /// Kept alive for exactly as long as the swap: the pointer written into the
    /// packet points into this allocation.
    replacement: Box<[u8]>,
}

impl PacketSwap {
    /// Swap `packet`'s payload for `replacement`.
    ///
    /// # Safety
    /// `packet` must be a live `CNetPacket` of the build `layout` was decoded
    /// from, and the guard must be dropped before that packet can be released.
    /// The caller owns the "drop before release" half; this function owns the
    /// write and [`Drop`] owns the restore.
    pub unsafe fn install(packet: *mut core::ffi::c_void, layout: Layout, replacement: Vec<u8>) -> Self {
        // SAFETY: the caller guarantees `packet` is a live CNetPacket of this
        // build's layout, so both slots are aligned, disjoint and inside it.
        let (data_slot, size_slot) = unsafe { (slot::<*const u8>(packet, layout.data), slot::<u32>(packet, layout.size)) };
        // SAFETY: both slots are the ones the layout names inside `packet`.
        let (original_data, original_size) = unsafe { (*data_slot, *size_slot) };
        let mut replacement = replacement.into_boxed_slice();
        // SAFETY: the pointer written is this guard's own allocation, which lives
        // until `Drop` restores the original values.
        unsafe {
            *data_slot = replacement.as_mut_ptr();
            *size_slot = u32::try_from(replacement.len()).unwrap_or(u32::MAX);
        }
        Self {
            data_slot,
            size_slot,
            original_data,
            original_size,
            replacement,
        }
    }

    /// How many bytes the original call is being shown, for the log line.
    pub fn served_size(&self) -> u32 {
        u32::try_from(self.replacement.len()).unwrap_or(u32::MAX)
    }
}

impl Drop for PacketSwap {
    fn drop(&mut self) {
        // SAFETY: the guard cannot outlive the packet callback that created it
        // (see `install`'s contract), and these are the two slots it wrote.
        unsafe {
            *self.data_slot = self.original_data;
            *self.size_slot = self.original_size;
        }
    }
}

/// One typed slot inside `packet`, at a byte offset the decoder produced.
///
/// # Safety
/// `packet` must be live for at least `offset + size_of::<T>()` bytes, and
/// `offset` must be aligned for `T` — [`Layout::new`] rejects a misaligned pair,
/// which is why it is the only constructor of the offsets used here.
unsafe fn slot<T>(packet: *mut core::ffi::c_void, offset: usize) -> *mut T {
    // SAFETY: the caller guarantees the offset and the live object.
    unsafe { packet.cast::<u8>().add(offset).cast::<T>() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentinel every byte outside the two slots keeps.
    const UNTOUCHED: u8 = 0xA5;

    /// A fake `CNetPacket`: 64 bytes, with a payload written into it.
    struct FakePacket {
        storage: Box<[u8; 64]>,
        payload: Vec<u8>,
    }

    impl FakePacket {
        fn new(layout: Layout, payload: &[u8]) -> Self {
            let mut fake = Self {
                storage: Box::new([UNTOUCHED; 64]),
                payload: payload.to_vec(),
            };
            // SAFETY: the storage is 64 bytes and the layout is validated to fit
            // inside it by `small_layout`.
            unsafe {
                let data = slot::<*const u8>(fake.as_ptr(), layout.data);
                let size = slot::<u32>(fake.as_ptr(), layout.size);
                *data = fake.payload.as_ptr();
                *size = fake.payload.len() as u32;
            }
            fake
        }

        fn as_ptr(&mut self) -> *mut core::ffi::c_void {
            self.storage.as_mut_ptr().cast()
        }

        fn read(&self, layout: Layout) -> (*const u8, u32) {
            // SAFETY: the storage is live and spans the layout's slots.
            unsafe {
                (
                    *slot::<*const u8>(self.storage.as_ptr() as *mut core::ffi::c_void, layout.data),
                    *slot::<u32>(self.storage.as_ptr() as *mut core::ffi::c_void, layout.size),
                )
            }
        }
    }

    fn small_layout() -> Layout {
        Layout::new(8, 24).expect("8-aligned pointer slot and 4-aligned size slot")
    }

    #[test]
    fn the_original_call_sees_the_replacement_and_steam_gets_its_own_back() {
        let layout = small_layout();
        let mut packet = FakePacket::new(layout, &[1, 2, 3]);
        let original = packet.read(layout);
        let replacement = vec![9u8, 8, 7, 6];

        {
            // SAFETY: the fake packet is live and drop order keeps it so.
            let swap = unsafe { PacketSwap::install(packet.as_ptr(), layout, replacement.clone()) };
            assert_eq!(swap.served_size(), 4);
            let (data, size) = packet.read(layout);
            assert_eq!(size, 4);
            // SAFETY: the swap owns four initialized bytes behind `data`.
            assert_eq!(unsafe { std::slice::from_raw_parts(data, size as usize) }, replacement);
        }

        let restored = packet.read(layout);
        assert_eq!(restored.0, original.0, "Steam's data pointer is back");
        assert_eq!(restored.1, original.1, "Steam's size is back");
        // SAFETY: the fake packet's own payload is still initialized.
        assert_eq!(unsafe { std::slice::from_raw_parts(restored.0, restored.1 as usize) }, [1, 2, 3]);
    }

    #[test]
    fn nothing_outside_the_two_decoded_slots_is_written() {
        let layout = small_layout();
        let mut packet = FakePacket::new(layout, &[1, 2, 3]);
        {
            // SAFETY: as above; the fake storage is 64 bytes wide.
            let _swap = unsafe { PacketSwap::install(packet.as_ptr(), layout, vec![0xff; 16]) };
        }
        for (offset, byte) in packet.storage.iter().enumerate() {
            let in_data = (layout.data..layout.data + 8).contains(&offset);
            let in_size = (layout.size..layout.size + 4).contains(&offset);
            if !in_data && !in_size {
                assert_eq!(*byte, UNTOUCHED, "byte {offset} is outside the layout");
            }
        }
    }

    #[test]
    fn a_larger_replacement_reports_its_own_size_and_is_restored_too() {
        let layout = small_layout();
        let mut packet = FakePacket::new(layout, &[1]);
        let original = packet.read(layout);
        {
            // SAFETY: as above.
            let swap = unsafe { PacketSwap::install(packet.as_ptr(), layout, vec![7; 32]) };
            assert_eq!(swap.served_size(), 32);
            assert_eq!(packet.read(layout).1, 32);
        }
        assert_eq!(packet.read(layout), original);
    }
}
