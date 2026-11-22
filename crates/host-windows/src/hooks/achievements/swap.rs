//! Swap-and-restore for one stats reply body.
//!
//! The geometry decoder says where the body is: the reply object at
//! `*(this + reply_field)`, and a `(pointer, length)` pair at `reply_object + body`
//! (see [`super::geometry`]). Replacing a reply is therefore the same discipline as
//! replacing a packet payload:
//!
//! 1. read the pair, keeping Steam's two values;
//! 2. write the replacement's pointer and length;
//! 3. call the displaced original;
//! 4. restore both values, unconditionally, in [`Drop`].
//!
//! # Why the pointer is swapped rather than copied in place
//!
//! A re-encoded protobuf is a different length, and the reply object's allocation
//! is Steam's. Writing a pointer to bytes this guard owns keeps Steam's buffer
//! untouched — nothing of Steam's is overwritten, so the restore is two writes and
//! cannot half-fail. The replacement outlives the original call because the guard
//! owns it.
//!
//! # What is never done
//!
//! No allocation happens while the pair is swapped, no lock is held, and nothing
//! outside the two derived slots is written.

use core::ffi::c_void;

use super::geometry::JobReply;

/// One installed reply swap: the replacement bytes and the two slots to restore.
pub struct ReplySwap {
    data_slot: *mut *const u8,
    size_slot: *mut usize,
    original_data: *const u8,
    original_size: usize,
    /// Kept alive for exactly as long as the swap: the pointer written into the
    /// reply object points into this allocation.
    replacement: Box<[u8]>,
}

impl ReplySwap {
    /// Install `replacement` as `this`'s reply body.
    ///
    /// # Safety
    /// `this` must be a live `CAPIJobRequestUserStats` of the build `geometry` was
    /// decoded from, and the guard must be dropped before that object is released.
    pub unsafe fn install(this: *mut c_void, geometry: JobReply, replacement: Vec<u8>) -> Option<Self> {
        if this.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `this` is a live job object of this build's
        // geometry, and `JobReply::new` validated that the field is pointer-aligned
        // and inside the object's bound.
        let reply_object = unsafe { *(this.cast::<u8>().add(geometry.reply_field).cast::<*const u8>()) };
        if reply_object.is_null() {
            return None;
        }
        // SAFETY: the reply object is live (it was just read from the job) and the
        // body slot is the decoded, aligned, bounded one.
        let data_slot = unsafe { reply_object.add(geometry.body).cast::<*const u8>().cast_mut() };
        // SAFETY: the length sits one pointer past the pointer slot, which is the
        // shape `geometry::JobReply::size_offset` derives.
        let size_slot = unsafe { reply_object.add(geometry.size_offset()).cast::<usize>().cast_mut() };
        // SAFETY: both slots are inside the same live reply object.
        let (original_data, original_size) = unsafe { (*data_slot, *size_slot) };
        if original_data.is_null() || original_size == 0 {
            // Nothing to replace: an empty reply is not one this host may rewrite.
            return None;
        }
        let mut replacement = replacement.into_boxed_slice();
        // SAFETY: the written pointer is this guard's own allocation, alive until
        // `Drop` restores the original pair.
        unsafe {
            *data_slot = replacement.as_mut_ptr();
            *size_slot = replacement.len();
        }
        Some(Self {
            data_slot,
            size_slot,
            original_data,
            original_size,
            replacement,
        })
    }

    /// How many bytes the original call is being shown.
    pub fn served_size(&self) -> usize {
        self.replacement.len()
    }
}

impl Drop for ReplySwap {
    fn drop(&mut self) {
        // SAFETY: the guard cannot outlive the job object that supplied the slots.
        unsafe {
            *self.data_slot = self.original_data;
            *self.size_slot = self.original_size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentinel every byte outside the two slots keeps.
    const UNTOUCHED: u8 = 0xA5;

    /// A fake job object plus a fake reply object, laid out the way the decoder
    /// describes them: the job holds a pointer at `reply_field`, the reply holds a
    /// `(pointer, length)` pair at `body`.
    struct FakeJob {
        job: Box<[u8; 0x40]>,
        reply: Box<[u8; 0x40]>,
        body: Vec<u8>,
    }

    fn geometry() -> JobReply {
        JobReply::new(0x10, 0x08).expect("aligned and in bounds")
    }

    impl FakeJob {
        fn new(body: &[u8]) -> Self {
            let mut fake = Self {
                job: Box::new([UNTOUCHED; 0x40]),
                reply: Box::new([UNTOUCHED; 0x40]),
                body: body.to_vec(),
            };
            let geometry = geometry();
            // SAFETY: both storages are wide enough for the offsets used here.
            unsafe {
                let reply_object = fake.reply.as_mut_ptr();
                *(fake.job.as_mut_ptr().add(geometry.reply_field).cast::<*mut u8>()) = reply_object;
                *(reply_object.add(geometry.body).cast::<*const u8>()) = fake.body.as_ptr();
                *(reply_object.add(geometry.size_offset()).cast::<usize>()) = fake.body.len();
            }
            fake
        }

        fn this(&mut self) -> *mut c_void {
            self.job.as_mut_ptr().cast()
        }

        /// The `(pointer, length)` pair Steam's reply object currently holds.
        fn slots(&self) -> (*const u8, usize) {
            let geometry = geometry();
            // SAFETY: both storages are live and span the offsets used here.
            unsafe {
                let reply_object = (self.reply.as_ptr() as *mut u8).cast::<u8>();
                (
                    *(reply_object.add(geometry.body).cast::<*const u8>()),
                    *(reply_object.add(geometry.size_offset()).cast::<usize>()),
                )
            }
        }

        fn served(&self) -> Vec<u8> {
            let (data, size) = self.slots();
            // SAFETY: the pointer is either the fixture's body or the swap's own
            // allocation, both valid for `size` bytes.
            unsafe { std::slice::from_raw_parts(data, size).to_vec() }
        }
    }

    #[test]
    fn the_original_call_sees_the_replacement_and_steam_gets_its_pair_back() {
        let mut fake = FakeJob::new(&[1, 2, 3]);
        let original = fake.slots();
        {
            // SAFETY: the fake job and reply outlive the guard.
            let swap = unsafe { ReplySwap::install(fake.this(), geometry(), vec![9, 8, 7, 6]) }
                .expect("a non-empty reply can be swapped");
            assert_eq!(swap.served_size(), 4);
            assert_eq!(fake.served(), vec![9, 8, 7, 6]);
        }
        assert_eq!(fake.slots(), original, "Steam's pointer and length are back");
        assert_eq!(fake.served(), vec![1, 2, 3]);
    }

    #[test]
    fn nothing_outside_the_two_decoded_slots_is_written() {
        let mut fake = FakeJob::new(&[1, 2, 3]);
        {
            // SAFETY: as above.
            let _swap = unsafe { ReplySwap::install(fake.this(), geometry(), vec![0xff; 32]) };
        }
        let geometry = geometry();
        for (offset, byte) in fake.reply.iter().enumerate() {
            let in_data = (geometry.body..geometry.body + 8).contains(&offset);
            let in_size = (geometry.size_offset()..geometry.size_offset() + 8).contains(&offset);
            if !in_data && !in_size {
                assert_eq!(*byte, UNTOUCHED, "reply byte {offset} is outside the slot pair");
            }
        }
        assert_eq!(fake.job[0], UNTOUCHED, "the job object is only read, never written");
    }

    #[test]
    fn a_null_job_or_a_null_reply_is_refused_rather_than_written() {
        // SAFETY: a null `this` is one of the documented inputs.
        assert!(unsafe { ReplySwap::install(core::ptr::null_mut(), geometry(), vec![1]) }.is_none());

        let mut fake = FakeJob::new(&[1, 2, 3]);
        // SAFETY: the job storage is live; the reply pointer is cleared first.
        unsafe {
            *(fake.job.as_mut_ptr().add(geometry().reply_field).cast::<*mut u8>()) =
                core::ptr::null_mut();
            assert!(ReplySwap::install(fake.this(), geometry(), vec![1]).is_none());
        }
    }

    #[test]
    fn an_empty_reply_body_is_left_alone() {
        let mut fake = FakeJob::new(&[]);
        // SAFETY: the storages are live; the length slot is zero.
        let swap = unsafe { ReplySwap::install(fake.this(), geometry(), vec![1, 2]) };
        assert!(swap.is_none(), "an empty reply is not one this host rewrites");
        assert_eq!(fake.served(), Vec::<u8>::new());
    }
}
