use super::*;
use iced_x86::code_asm::*;

/// Each region holds `Alloc`, `Init` and `Release` at these offsets.
const ALLOC: usize = 0x00;
const INIT: usize = 0x80;
const RELEASE: usize = 0x180;

/// Calls that leave the region: allocator lock, constructor, PIC thunk.
const EXTERNAL: u64 = 0x0100_0000;
/// Displacements off the i686 PIC base; any full imm32 will do.
const PIC_ADD: i32 = 0x003b_eb35;
const GLOBAL: i32 = 0x08cc;

const fn api(
    [conn_id, data, size, owned_data]: [usize; 4],
    refcount: usize,
    object_size: usize,
) -> PacketApi {
    PacketApi {
        fields: PacketFields {
            conn_id,
            data,
            size,
            owned_data,
        },
        refcount,
        object_size,
    }
}

/// The layouts either side of client build 1790380355.
const X86_LAYOUT: PacketApi = api([0x00, 0x04, 0x08, 0x10], 0x0c, 0x20);
const X86_GROWN_LAYOUT: PacketApi = api([0x00, 0x0c, 0x10, 0x18], 0x14, 0x38);
const X64_LAYOUT: PacketApi = api([0x00, 0x08, 0x10, 0x18], 0x14, 0x30);
const X64_GROWN_LAYOUT: PacketApi = api([0x00, 0x10, 0x18, 0x20], 0x1c, 0x58);
/// The layout both live Windows builds measure, and the one the MS-x64
/// shapes below are built for.
const MSX64_LAYOUT: PacketApi = X64_GROWN_LAYOUT;

fn displacement(offset: usize) -> i32 {
    offset as i32
}

/// `Alloc` through Steam's allocator interface, with the source line as a
/// second constant argument that must not be taken for the size.
fn x86_alloc(layout: &PacketApi) -> Vec<u8> {
    assemble(32, |a| {
        a.push(esi)?;
        a.push(ebx)?;
        a.call(EXTERNAL)?;
        a.add(ebx, PIC_ADD)?;
        a.sub(esp, 8)?;
        a.mov(eax, dword_ptr(ebx + GLOBAL))?;
        a.mov(eax, dword_ptr(eax))?;
        a.lea(ecx, ptr(ebx - 0x0010_0000))?;
        a.mov(edx, dword_ptr(eax))?;
        a.push(1)?;
        a.push(0)?;
        a.push(0x7bf)?;
        a.push(ecx)?;
        a.push(layout.object_size as i32)?;
        a.push(eax)?;
        a.call(dword_ptr(edx + 0x14))?;
        a.add(esp, 0x18)?;
        a.sub(esp, 0x0c)?;
        a.push(eax)?;
        a.mov(esi, eax)?;
        a.call(EXTERNAL)?;
        a.add(esp, 0x18)?;
        a.mov(eax, esi)?;
        a.pop(ebx)?;
        a.pop(esi)?;
        a.ret()
    })
}

fn x64_alloc(layout: &PacketApi) -> Vec<u8> {
    assemble(64, |a| {
        a.push(rbx)?;
        a.mov(rax, qword_ptr(rbx + GLOBAL))?;
        a.mov(r9d, 1)?;
        a.xor(r8d, r8d)?;
        a.mov(ecx, 0x7bf)?;
        a.mov(esi, layout.object_size as u32)?;
        a.mov(rdi, qword_ptr(rax))?;
        a.mov(rax, qword_ptr(rdi))?;
        a.call(qword_ptr(rax + 0x28))?;
        a.mov(rbx, rax)?;
        a.mov(rdi, rax)?;
        a.call(EXTERNAL)?;
        a.mov(rax, rbx)?;
        a.pop(rbx)?;
        a.ret()
    })
}

/// i686 `Init` with a frame pointer: arguments read through ebp and
/// relayed through eax. `spill_data` is the steamrt build, which parks the
/// data argument in a local first and stores it from there.
fn x86_frame_init(layout: &PacketApi, spill_data: bool) -> Vec<u8> {
    let fields = layout.fields;
    let arg = |index: i32| ebp + (8 + 4 * index);
    assemble(32, |a| {
        let mut out = a.create_label();
        a.push(ebp)?;
        a.mov(ebp, esp)?;
        a.push(edi)?;
        a.call(EXTERNAL)?;
        a.add(edi, PIC_ADD)?;
        a.push(esi)?;
        a.push(ebx)?;
        a.sub(esp, 0x2c)?;
        // add_ref, or the data pointer on steamrt, kept in a local.
        a.mov(eax, dword_ptr(arg(if spill_data { 2 } else { 5 })))?;
        a.mov(esi, dword_ptr(arg(0)))?;
        a.mov(dword_ptr(ebp - 0x2c), eax)?;
        a.test(eax, eax)?;
        a.je(out)?;
        a.mov(ecx, dword_ptr(arg(3)))?;
        a.test(ecx, ecx)?;
        a.je(out)?;
        a.mov(eax, dword_ptr(arg(1)))?;
        a.mov(dword_ptr(esi + displacement(layout.object_size - 4)), 0)?;
        a.mov(dword_ptr(esi + displacement(fields.conn_id)), eax)?;
        if spill_data {
            a.mov(eax, dword_ptr(ebp - 0x2c))?;
        } else {
            a.mov(eax, dword_ptr(arg(2)))?;
        }
        a.mov(dword_ptr(esi + displacement(fields.data)), eax)?;
        a.mov(eax, dword_ptr(arg(3)))?;
        a.mov(dword_ptr(esi + displacement(fields.size)), eax)?;
        a.mov(eax, dword_ptr(arg(4)))?;
        a.mov(dword_ptr(esi + displacement(fields.owned_data)), eax)?;
        a.set_label(&mut out)?;
        a.lea(esp, ptr(ebp - 0x0c))?;
        a.pop(ebx)?;
        a.pop(esi)?;
        a.pop(edi)?;
        a.pop(ebp)?;
        a.ret()
    })
}

/// i686 `Init` without a frame pointer: arguments read through esp once
/// the frame is set up, and stored straight from the registers holding them.
fn x86_frameless_init(layout: &PacketApi) -> Vec<u8> {
    let fields = layout.fields;
    // Four pushes, the frame, and the return address.
    let arg = |index: i32| esp + (0x1c + 0x10 + 4 + 4 * index);
    assemble(32, |a| {
        let mut out = a.create_label();
        a.push(ebp)?;
        a.push(edi)?;
        a.push(esi)?;
        a.push(ebx)?;
        a.call(EXTERNAL)?;
        a.add(ebx, PIC_ADD)?;
        a.sub(esp, 0x1c)?;
        a.mov(eax, dword_ptr(arg(5)))?;
        a.mov(ebp, dword_ptr(arg(2)))?;
        a.mov(esi, dword_ptr(arg(0)))?;
        a.mov(ecx, dword_ptr(arg(1)))?;
        a.mov(edi, dword_ptr(arg(3)))?;
        a.mov(dword_ptr(esp + 4), eax)?;
        a.mov(edx, dword_ptr(arg(4)))?;
        a.test(ebp, ebp)?;
        a.je(out)?;
        a.test(edi, edi)?;
        a.je(out)?;
        a.mov(dword_ptr(esi + displacement(fields.conn_id)), ecx)?;
        a.mov(dword_ptr(esi + displacement(fields.owned_data)), edx)?;
        a.mov(dword_ptr(esi + displacement(fields.data)), ebp)?;
        a.mov(dword_ptr(esi + displacement(fields.size)), edi)?;
        a.mov(dword_ptr(esi + displacement(layout.object_size - 4)), 0)?;
        a.set_label(&mut out)?;
        a.add(esp, 0x1c)?;
        a.pop(ebx)?;
        a.pop(esi)?;
        a.pop(edi)?;
        a.pop(ebp)?;
        a.ret()
    })
}

/// x86_64 `Init`: arguments parked in callee-saved registers, one of which
/// first holds the stack guard, with a computed value stored beside them.
fn x64_init(layout: &PacketApi) -> Vec<u8> {
    let fields = layout.fields;
    assemble(64, |a| {
        let mut out = a.create_label();
        a.push(r15)?;
        a.mov(r15d, esi)?;
        a.push(r14)?;
        a.mov(r14, r8)?;
        a.push(r13)?;
        a.push(r12)?;
        a.mov(r12, rdx)?;
        a.push(rbp)?;
        a.mov(ebp, ecx)?;
        a.push(rbx)?;
        a.mov(rbx, rdi)?;
        a.sub(rsp, 0x18)?;
        a.test(rdx, rdx)?;
        a.mov(r13, qword_ptr(0x28).fs())?;
        a.mov(qword_ptr(rsp + 8), r13)?;
        a.mov(r13d, r9d)?;
        a.je(out)?;
        a.test(ebp, ebp)?;
        a.je(out)?;
        a.mov(dword_ptr(rbx + displacement(fields.conn_id)), r15d)?;
        a.mov(qword_ptr(rbx + displacement(fields.data)), r12)?;
        a.mov(dword_ptr(rbx + displacement(fields.size)), ebp)?;
        a.mov(qword_ptr(rbx + displacement(fields.owned_data)), r14)?;
        a.mov(qword_ptr(rbx + displacement(layout.object_size - 8)), 0)?;
        a.movzx(eax, byte_ptr(r12))?;
        a.mov(dword_ptr(rbx + displacement(fields.conn_id + 4)), eax)?;
        a.set_label(&mut out)?;
        a.add(rsp, 0x18)?;
        a.pop(rbx)?;
        a.pop(rbp)?;
        a.pop(r12)?;
        a.pop(r13)?;
        a.pop(r14)?;
        a.pop(r15)?;
        a.ret()
    })
}

/// i686 `Release` behind a full prologue, so `this` is read off the stack.
fn x86_release(layout: &PacketApi) -> Vec<u8> {
    assemble(32, |a| {
        let mut free = a.create_label();
        a.push(ebp)?;
        a.push(edi)?;
        a.push(esi)?;
        a.push(ebx)?;
        a.call(EXTERNAL)?;
        a.add(ebx, PIC_ADD)?;
        a.sub(esp, 0x2c)?;
        a.mov(edi, dword_ptr(esp + 0x40))?;
        a.sub(dword_ptr(edi + displacement(layout.refcount)), 1)?;
        a.je(free)?;
        a.add(esp, 0x2c)?;
        a.pop(ebx)?;
        a.pop(esi)?;
        a.pop(edi)?;
        a.pop(ebp)?;
        a.ret()?;
        a.set_label(&mut free)?;
        a.ud2()
    })
}

/// The compiler shapes seen across client builds, each with the layout it
/// was observed with.
fn shapes() -> [(&'static str, u32, Vec<u8>, PacketApi); 5] {
    let x86 = |init: fn(&PacketApi) -> Vec<u8>, layout: PacketApi| {
        region(&x86_alloc(&layout), &init(&layout), &x86_release(&layout))
    };
    let x64 = |layout: PacketApi| {
        region(
            &x64_alloc(&layout),
            &x64_init(&layout),
            &synthetic_release(displacement(layout.refcount)),
        )
    };
    [
        (
            "i686 with a frame pointer",
            32,
            x86(|layout| x86_frame_init(layout, false), X86_LAYOUT),
            X86_LAYOUT,
        ),
        (
            "i686 steamrt, data argument spilled",
            32,
            x86(|layout| x86_frame_init(layout, true), X86_LAYOUT),
            X86_LAYOUT,
        ),
        (
            "i686 without a frame pointer",
            32,
            x86(x86_frameless_init, X86_GROWN_LAYOUT),
            X86_GROWN_LAYOUT,
        ),
        ("x86_64", 64, x64(X64_LAYOUT), X64_LAYOUT),
        ("x86_64 grown", 64, x64(X64_GROWN_LAYOUT), X64_GROWN_LAYOUT),
    ]
}

/// MS-x64 `Init`: `(this, conn_id, data, size, owned_data, add_ref)` in
/// `rcx, edx, r8, r9d`, with the owned buffer past the shadow space. The
/// compiler copies each argument into a callee-saved register first and
/// stores them in a different order from the one they arrive in.
fn msx64_init(layout: &PacketApi) -> Vec<u8> {
    let fields = layout.fields;
    assemble(64, |a| {
        a.push(rbx)?;
        a.sub(rsp, 0x30)?;
        a.mov(r10d, r9d)?;
        a.mov(rsi, r8)?;
        a.mov(edi, edx)?;
        a.mov(rbx, rcx)?;
        a.mov(rax, qword_ptr(rsp + 0x60))?;
        a.mov(qword_ptr(rbx + displacement(fields.owned_data)), rax)?;
        a.mov(dword_ptr(rbx + displacement(fields.size)), r10d)?;
        a.mov(qword_ptr(rbx + displacement(fields.data)), rsi)?;
        a.mov(dword_ptr(rbx + displacement(fields.conn_id)), edi)?;
        a.mov(qword_ptr(rbx + displacement(layout.object_size - 8)), 0)?;
        a.add(rsp, 0x30)?;
        a.pop(rbx)?;
        a.ret()
    })
}

/// MS-x64 `Alloc`: the size is a constant in `rdx`, `this` is copied into
/// `rcx`, and the allocator is called indirectly. The prologue steps over a
/// `lock`-prefixed counter with an unconditional jump, which the walk has
/// to follow to reach the allocation at all.
fn msx64_alloc(layout: &PacketApi) -> Vec<u8> {
    assemble(64, |a| {
        let mut skip = a.create_label();
        let mut go = a.create_label();
        let mut constructor = a.create_label();
        a.push(rbx)?;
        a.sub(rsp, 0x20)?;
        a.mov(esi, edx)?;
        a.xor(eax, eax)?;
        a.cmp(eax, 8)?;
        a.jl(skip)?;
        a.jmp(go)?;
        a.set_label(&mut skip)?;
        a.int3()?;
        a.set_label(&mut go)?;
        a.mov(edx, layout.object_size as u32)?;
        a.mov(rcx, rbx)?;
        a.mov(rax, qword_ptr(rcx))?;
        a.call(qword_ptr(rax + 0x10))?;
        a.mov(rcx, rax)?;
        a.mov(rbx, rax)?;
        a.call(constructor)?;
        a.mov(rax, rbx)?;
        a.add(rsp, 0x20)?;
        a.pop(rbx)?;
        a.ret()?;
        a.set_label(&mut constructor)?;
        a.ret()
    })
}

/// MS-x64 `Release`: the refcount decrement is `[rcx + offset]`.
fn msx64_release(layout: &PacketApi) -> Vec<u8> {
    assemble(64, |a| {
        a.push(rsi)?;
        a.sub(rsp, 0x20)?;
        a.sub(dword_ptr(rcx + displacement(layout.refcount)), 1)?;
        a.mov(rsi, rcx)?;
        a.add(rsp, 0x20)?;
        a.pop(rsi)?;
        a.ret()
    })
}

fn msx64_region(layout: PacketApi) -> Vec<u8> {
    region(
        &msx64_alloc(&layout),
        &msx64_init(&layout),
        &msx64_release(&layout),
    )
}

fn pe_segment(code: &[u8], bits: u8) -> CodeSegment<'_> {
    CodeSegment {
        bytes: code,
        file_offset: 0,
        vaddr: 0x1000,
        bits,
        container: Container::Pe,
    }
}

#[test]
fn decodes_the_msx64_packet_layout() {
    let code = msx64_region(MSX64_LAYOUT);
    assert_eq!(
        decode_api_in(Abi::MsX64, 64, &code, ALLOC, INIT, RELEASE),
        Ok(MSX64_LAYOUT)
    );
}

#[test]
fn decodes_a_pe_segment_but_not_the_same_code_as_elf() {
    let code = msx64_region(MSX64_LAYOUT);
    let pe = pe_segment(&code, 64);
    assert_eq!(convention(&pe), Abi::MsX64);
    assert_eq!(decode_api_segment(&pe, ALLOC, INIT, RELEASE), Ok(MSX64_LAYOUT));
    // The same bytes under the System V convention are a different ABI:
    // `Init`'s this-pointer is `rcx` here, not `rdi`.
    let elf = CodeSegment {
        container: Container::Elf,
        ..pe
    };
    assert_eq!(convention(&elf), Abi::SysV);
    assert!(decode_api_segment(&elf, ALLOC, INIT, RELEASE).is_err());
    assert_eq!(convention(&pe_segment(&code, 32)), Abi::SysV);
}

#[test]
fn following_a_forward_jump_is_what_reaches_the_allocation() {
    let code = msx64_alloc(&MSX64_LAYOUT);
    assert_eq!(decode_alloc_in(Abi::MsX64, 64, &code, 0), Ok(0x58));
    // The same walk without that allowance stops at the `jmp`.
    let linear = arg_flow::trace_with(
        Walk::linear(Abi::MsX64, ALLOC_WINDOW_MSX64),
        64,
        &code,
        0,
    );
    assert!(linear.calls.is_empty());
}

#[test]
fn decodes_every_compiler_shape() {
    for (shape, bitness, code, expected) in shapes() {
        assert_eq!(
            decode_api(bitness, &code, ALLOC, INIT, RELEASE),
            Ok(expected),
            "{shape}"
        );
    }
}

#[test]
fn rejects_each_function_in_another_role() {
    for (shape, bitness, code, _) in shapes() {
        for offset in [ALLOC, RELEASE] {
            assert!(decode_init(bitness, &code, offset).is_err(), "{shape}");
        }
        for offset in [ALLOC, INIT] {
            assert!(decode_release(bitness, &code, offset).is_err(), "{shape}");
        }
        for offset in [INIT, RELEASE] {
            assert!(decode_alloc(bitness, &code, offset).is_err(), "{shape}");
        }
    }
}

fn assemble(
    bitness: u32,
    build: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut assembler = CodeAssembler::new(bitness).unwrap();
    build(&mut assembler).unwrap();
    assembler.assemble(0).unwrap()
}

fn synthetic_alloc(size: u32) -> Vec<u8> {
    assemble(64, |a| {
        let mut constructor = a.create_label();
        a.push(rbx)?;
        a.mov(esi, size)?;
        a.call(qword_ptr(rax + 0x28))?;
        a.mov(rbx, rax)?;
        a.mov(rdi, rax)?;
        a.call(constructor)?;
        a.mov(rax, rbx)?;
        a.pop(rbx)?;
        a.ret()?;
        a.set_label(&mut constructor)?;
        a.ret()
    })
}

fn synthetic_init(extra: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    assemble(64, |a| {
        a.mov(dword_ptr(rdi), esi)?;
        a.mov(qword_ptr(rdi + 0x08), rdx)?;
        a.mov(dword_ptr(rdi + 0x10), ecx)?;
        a.mov(qword_ptr(rdi + 0x18), r8)?;
        extra(a)?;
        a.ret()
    })
}

fn synthetic_release(refcount: i32) -> Vec<u8> {
    assemble(64, |a| {
        a.sub(dword_ptr(rdi + refcount), 1)?;
        a.ret()
    })
}

/// Lay the three functions out at the region offsets.
fn region(alloc: &[u8], init: &[u8], release: &[u8]) -> Vec<u8> {
    let mut code = vec![0xcc; RELEASE + 0x40];
    code[ALLOC..ALLOC + alloc.len()].copy_from_slice(alloc);
    code[INIT..INIT + init.len()].copy_from_slice(init);
    code[RELEASE..RELEASE + release.len()].copy_from_slice(release);
    code
}

#[test]
fn decodes_a_consistent_synthetic_packet() {
    let code = region(
        &synthetic_alloc(0x30),
        &synthetic_init(|_| Ok(())),
        &synthetic_release(0x14),
    );
    assert_eq!(
        decode_api(64, &code, ALLOC, INIT, RELEASE),
        Ok(api([0x00, 0x08, 0x10, 0x18], 0x14, 0x30))
    );
}

#[test]
fn rejects_an_argument_stored_at_two_offsets() {
    let init = synthetic_init(|a| a.mov(qword_ptr(rdi + 0x20), rdx));
    assert_eq!(decode_init(64, &init, 0), Err("data pointer store"));
}

#[test]
fn rejects_a_refcount_inside_a_field() {
    let code = region(
        &synthetic_alloc(0x30),
        &synthetic_init(|_| Ok(())),
        &synthetic_release(0x10),
    );
    assert_eq!(
        decode_api(64, &code, ALLOC, INIT, RELEASE),
        Err("refcount apart from the packet fields")
    );
}

#[test]
fn rejects_fields_past_the_allocation() {
    let code = region(
        &synthetic_alloc(0x18),
        &synthetic_init(|_| Ok(())),
        &synthetic_release(0x14),
    );
    assert_eq!(
        decode_api(64, &code, ALLOC, INIT, RELEASE),
        Err("packet fields inside the allocation")
    );
}
