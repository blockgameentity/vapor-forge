use super::*;
use crate::scan::{CodeSegment, Container};
use iced_x86::code_asm::*;

/// A segment whose bytes start at `vaddr`; instruction addresses in the walk are
/// absolute, so the assembler's relative branches resolve inside it.
pub(super) fn segment(bytes: &[u8], bits: u8, container: Container) -> CodeSegment<'_> {
    CodeSegment {
        bytes,
        file_offset: 0,
        vaddr: 0x1000,
        bits,
        container,
    }
}

pub(super) fn ms_x64(bytes: &[u8]) -> CodeSegment<'_> {
    segment(bytes, 64, Container::Pe)
}

pub(super) fn sysv(bytes: &[u8]) -> CodeSegment<'_> {
    segment(bytes, 64, Container::Elf)
}

pub(super) fn assemble(
    bitness: u32,
    build: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut assembler = CodeAssembler::new(bitness).unwrap();
    build(&mut assembler).unwrap();
    assembler.assemble(0).unwrap()
}

/// `BInit`'s shape on MS-x64: `this` arrives in `rcx`, is parked in a
/// callee-saved register, and the reply load and body `lea` are separated by the
/// null test and a store.
pub(super) fn ms_x64_body() -> Vec<u8> {
    assemble(64, |a| {
        a.push(rbx)?;
        a.mov(rbx, rcx)?;
        a.mov(rcx, qword_ptr(rbx + 0x1b0))?;
        a.test(rcx, rcx)?;
        a.mov(qword_ptr(rsp + 0x60), rax)?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.cmove(rdi, r15)?;
        a.mov(rsi, rdi)?;
        a.ret()?;
        Ok(())
    })
}

#[test]
fn ms_x64_reads_the_reply_field_and_body() {
    let bytes = ms_x64_body();
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Ok(JobReply {
            reply_field: 0x1b0,
            body: 0x18
        })
    );
}

#[test]
fn sys_v_reads_the_same_shape_from_its_own_argument_register() {
    // System V hands `this` in `rdi`, and the load goes straight from it.
    let bytes = assemble(64, |a| {
        a.push(rbx)?;
        a.mov(rax, qword_ptr(rdi + 0x2a0))?;
        a.test(rax, rax)?;
        a.lea(rsi, qword_ptr(rax + 0x20))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&sysv(&bytes), 0),
        Ok(JobReply {
            reply_field: 0x2a0,
            body: 0x20
        })
    );
}

#[test]
fn cdecl_reads_the_receiver_from_its_stack_slot() {
    // 32-bit cdecl: `this` is the first stacked argument, reached through the
    // frame pointer, and the reply pointer is four bytes wide.
    let bytes = assemble(32, |a| {
        a.push(ebp)?;
        a.mov(ebp, esp)?;
        a.push(ebx)?;
        a.mov(ebx, dword_ptr(ebp + 8))?;
        a.mov(ecx, dword_ptr(ebx + 0x1b0))?;
        a.lea(edi, dword_ptr(ecx + 0x18))?;
        a.pop(ebx)?;
        a.pop(ebp)?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&segment(&bytes, 32, Container::Pe), 0),
        Ok(JobReply {
            reply_field: 0x1b0,
            body: 0x18
        })
    );
}

#[test]
fn follows_the_call_that_is_handed_this() {
    // The measured `BInit` shape: the parser call sits on the arm a linear walk
    // jumps over, and the body `lea` is inside the parser.
    let bytes = assemble(64, |a| {
        let mut call_site = a.create_label();
        let mut after = a.create_label();
        let mut parser = a.create_label();
        a.push(rbx)?;
        a.mov(rbx, rcx)?;
        a.test(al, al)?;
        a.je(call_site)?;
        a.mov(edi, 1)?;
        a.jmp(after)?;
        a.set_label(&mut call_site)?;
        a.mov(rcx, rbx)?;
        a.call(parser)?;
        a.set_label(&mut after)?;
        a.ret()?;
        a.set_label(&mut parser)?;
        a.mov(r14, rcx)?;
        a.mov(rcx, qword_ptr(r14 + 0x1a8))?;
        a.test(rcx, rcx)?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Ok(JobReply {
            reply_field: 0x1a8,
            body: 0x18
        })
    );
}

#[test]
fn the_same_pair_from_the_entry_and_its_callee_is_one_answer() {
    // The entry carries the shape and so does the parser it calls: the pair is
    // recorded twice and still describes one layout.
    let bytes = assemble(64, |a| {
        let mut parser = a.create_label();
        a.push(rbx)?;
        a.mov(rbx, rcx)?;
        a.mov(rcx, qword_ptr(rbx + 0x1b0))?;
        a.lea(rdi, qword_ptr(rcx + 0x18))?;
        a.mov(rcx, rbx)?;
        a.call(parser)?;
        a.ret()?;
        a.set_label(&mut parser)?;
        a.mov(r14, rcx)?;
        a.mov(rcx, qword_ptr(r14 + 0x1b0))?;
        a.lea(rsi, qword_ptr(rcx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&ms_x64(&bytes), 0),
        Ok(JobReply {
            reply_field: 0x1b0,
            body: 0x18
        })
    );
}

#[test]
fn a_body_lea_ends_the_register_that_held_this() {
    // System V: `rdi` arrives as `this` and the body `lea` overwrites it with a
    // pointer pulled out of the reply. A load through it afterwards is not a
    // member of the job, so it must not become a second candidate.
    let bytes = assemble(64, |a| {
        a.mov(rax, qword_ptr(rdi + 0x2a0))?;
        a.lea(rdi, qword_ptr(rax + 0x20))?;
        a.mov(rdx, qword_ptr(rdi + 0x100))?;
        a.lea(rsi, qword_ptr(rdx + 0x18))?;
        a.ret()?;
        Ok(())
    });
    assert_eq!(
        decode_job_reply(&sysv(&bytes), 0),
        Ok(JobReply {
            reply_field: 0x2a0,
            body: 0x20
        })
    );
}

mod rejections;
