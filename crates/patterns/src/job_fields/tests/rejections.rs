//! The shapes the decoder must refuse.
use super::{assemble, ms_x64, ms_x64_body};
use crate::job_fields::decode_job_reply;
use iced_x86::code_asm::*;

#[test]
fn ignores_a_call_that_is_not_handed_this() {
    // The same layout, but the callee receives a fresh object: its reply field
    // describes something else and must not be taken.
    let bytes = assemble(64, |a| {
        let mut parser = a.create_label();
        a.push(rbx)?;
        a.mov(rbx, rcx)?;
        a.call(qword_ptr(rax + 0x28))?;
        a.mov(rcx, rax)?;
        a.call(parser)?;
        a.ret()?;
        a.set_label(&mut parser)?;
        a.mov(rcx, qword_ptr(rcx + 0x1a8))?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}

#[test]
fn rejects_when_nothing_loads_a_reply_pointer() {
    let bytes = assemble(64, |a| {
        a.mov(rax, rcx)?;
        a.xor(eax, eax)?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}

#[test]
fn rejects_two_different_candidates() {
    let bytes = assemble(64, |a| {
        a.mov(rbx, rcx)?;
        a.mov(rcx, qword_ptr(rbx + 0x1b0))?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.mov(rdx, qword_ptr(rbx + 0x2a0))?;
        a.lea(rsi, qword_ptr(rdx + 0x20))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("ambiguous reply field")
    );
}

#[test]
fn rejects_a_body_lea_outside_the_window() {
    let bytes = assemble(64, |a| {
        a.mov(rcx, qword_ptr(rcx + 0x1b0))?;
        for _ in 0..0x50 {
            a.nop()?;
        }
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply body outside the window")
    );
}

#[test]
fn rejects_a_chain_whose_register_is_overwritten() {
    let bytes = assemble(64, |a| {
        a.mov(rcx, qword_ptr(rcx + 0x1b0))?;
        a.mov(rcx, rdx)?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}

#[test]
fn rejects_a_narrow_load_in_a_64_bit_image() {
    // `mov ecx, [rcx+0x1b0]` zero-extends, so it is not the reply pointer.
    let bytes = assemble(64, |a| {
        a.mov(ecx, dword_ptr(rcx + 0x1b0))?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}

#[test]
fn rejects_a_field_outside_the_job_object() {
    let bytes = assemble(64, |a| {
        a.mov(rcx, qword_ptr(rcx + 0x2))?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field outside the job object")
    );
}

#[test]
fn rejects_an_entry_outside_the_segment() {
    let bytes = ms_x64_body();
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), bytes.len() + 1),
        Err("entry outside the code segment")
    );
}

#[test]
fn does_not_take_a_derived_pointer_for_this() {
    // `lea r14, [rcx+0x190]` is an interior pointer; a load through it is not a
    // member of the job and must not anchor the shape.
    let bytes = assemble(64, |a| {
        a.lea(r14, qword_ptr(rcx + 0x190))?;
        a.mov(rdx, qword_ptr(r14 + 0x20))?;
        a.lea(rdi, qword_ptr(rdx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}

#[test]
fn does_not_keep_this_across_a_call_that_destroys_it() {
    // `this` is in `rcx`, which an MS-x64 call may destroy.
    let bytes = assemble(64, |a| {
        a.call(qword_ptr(rax + 0x28))?;
        a.mov(rdx, qword_ptr(rcx + 0x1b0))?;
        a.lea(rdi, qword_ptr(rdx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Err("reply field load")
    );
}
