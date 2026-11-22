//! Field offsets of Steam's `CNetPacket`, read from the functions that fill it.
//!
//! These fields sit at different offsets in different builds, so they are not
//! pinned. `Init` stores its arguments straight into the packet, `Release`
//! decrements the refcount first thing, and `Alloc` passes the object size to
//! Steam's allocator; each of those is stable in meaning even where the code
//! around it is not.
//!
//! The runtime and the offline scan both decode through here, so what the scan
//! reports is what the hooks will use.
//!
//! Two builds, two argument conventions: the Linux client is System V and the
//! Windows one is MS-x64, where `Init` arrives as
//! `(this, conn_id, data, size, owned_data, add_ref)` in `rcx, edx, r8, r9d` with
//! the rest pushed past the shadow space. The plain decoders are System V — the
//! Linux callers are unchanged — and [`decode_api_segment`] picks the convention
//! from the [`CodeSegment`] it is given, so a PE section decodes as MS-x64.

use crate::arg_flow::{self, Abi, Value, Walk};
use crate::scan::{CodeSegment, Container};
use iced_x86::Mnemonic;

const INIT_WINDOW: usize = 0x200;
const RELEASE_WINDOW: usize = 0x40;
const ALLOC_WINDOW: usize = 0x80;
/// The same walk for an MS-x64 `Alloc`: the Windows builds open with
/// `DCHECK`s whose failure paths sit between the entry and the allocator call,
/// and one of them steps over a `lock`-prefixed counter, so the walk needs the
/// longer window and has to follow that forward jump. Measured: 0x106 bytes to
/// the `ret` in the beta build, 0xe1 in the public one.
const ALLOC_WINDOW_MSX64: usize = 0x180;
/// Well past the largest packet seen, and small enough to reject a store into
/// something that is not a packet.
const MAX_FIELD_END: usize = 0x100;
const MIN_OBJECT_SIZE: usize = 0x10;
const MAX_OBJECT_SIZE: usize = 0x400;

/// Offsets `CNetPacket::Init(packet, conn_id, data, size, owned_data, add_ref)`
/// stores its arguments at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketFields {
    pub conn_id: usize,
    pub data: usize,
    pub size: usize,
    pub owned_data: usize,
}

impl PacketFields {
    /// Offset and width of each field, for a target with `pointer_size`-byte pointers.
    pub fn spans(&self, pointer_size: usize) -> [(usize, usize); 4] {
        [
            (self.conn_id, 4),
            (self.data, pointer_size),
            (self.size, 4),
            (self.owned_data, pointer_size),
        ]
    }

    /// One past the last byte any field occupies.
    pub fn end(&self, pointer_size: usize) -> usize {
        self.spans(pointer_size)
            .iter()
            .map(|(offset, width)| offset + width)
            .max()
            .unwrap_or(0)
    }
}

/// Everything native packet injection relies on, checked against itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketApi {
    pub fields: PacketFields,
    pub refcount: usize,
    pub object_size: usize,
}

/// Read the field offsets from `CNetPacket::Init` at `offset` in `code`.
pub fn decode_init(bitness: u32, code: &[u8], offset: usize) -> Result<PacketFields, &'static str> {
    decode_init_in(Abi::SysV, bitness, code, offset)
}

/// [`decode_init`] for a named call convention.
pub fn decode_init_in(
    abi: Abi,
    bitness: u32,
    code: &[u8],
    offset: usize,
) -> Result<PacketFields, &'static str> {
    let pointer_size = bitness as usize / 8;
    let trace = arg_flow::trace_with(Walk::linear(abi, INIT_WINDOW), bitness, code, offset);
    let field = |argument: u8, width: usize, label: &'static str| {
        let mut offsets: Vec<(i64, usize)> = trace
            .stores
            .iter()
            .filter(|store| {
                store.object == Value::Arg(0) && store.value == Some(Value::Arg(argument))
            })
            .map(|store| (store.offset, store.width))
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        // Two different destinations for one argument means the walk crossed
        // into code it does not understand; neither can be trusted.
        match offsets[..] {
            [(offset, stored)] if stored == width => usize::try_from(offset)
                .ok()
                .filter(|offset| offset % width == 0 && offset + width <= MAX_FIELD_END)
                .ok_or(label),
            _ => Err(label),
        }
    };
    let fields = PacketFields {
        conn_id: field(1, 4, "connection id store")?,
        data: field(2, pointer_size, "data pointer store")?,
        size: field(3, 4, "size store")?,
        owned_data: field(4, pointer_size, "owned buffer store")?,
    };
    if overlaps(&fields.spans(pointer_size)) {
        return Err("distinct packet fields");
    }
    Ok(fields)
}

/// Read the refcount offset from `CNetPacket::Release` at `offset` in `code`.
pub fn decode_release(bitness: u32, code: &[u8], offset: usize) -> Result<usize, &'static str> {
    decode_release_in(Abi::SysV, bitness, code, offset)
}

/// [`decode_release`] for a named call convention.
pub fn decode_release_in(
    abi: Abi,
    bitness: u32,
    code: &[u8],
    offset: usize,
) -> Result<usize, &'static str> {
    const LABEL: &str = "refcount decrement";
    let trace = arg_flow::trace_with(Walk::linear(abi, RELEASE_WINDOW), bitness, code, offset);
    let update = trace
        .updates
        .iter()
        .find(|update| update.object == Value::Arg(0))
        .ok_or(LABEL)?;
    let decrements = match update.mnemonic {
        Mnemonic::Sub => update.operand == Some(1),
        Mnemonic::Add => update.operand == Some(-1),
        Mnemonic::Dec => true,
        _ => false,
    };
    if !decrements || update.width != 4 {
        return Err(LABEL);
    }
    usize::try_from(update.offset)
        .ok()
        .filter(|offset| offset % 4 == 0 && offset + 4 <= MAX_FIELD_END)
        .ok_or(LABEL)
}

/// Read the object size from `CNetPacket::Alloc` at `offset` in `code`.
pub fn decode_alloc(bitness: u32, code: &[u8], offset: usize) -> Result<usize, &'static str> {
    decode_alloc_in(Abi::SysV, bitness, code, offset)
}

/// [`decode_alloc`] for a named call convention.
pub fn decode_alloc_in(
    abi: Abi,
    bitness: u32,
    code: &[u8],
    offset: usize,
) -> Result<usize, &'static str> {
    let trace = arg_flow::trace_with(alloc_walk(abi), bitness, code, offset);
    // Steam allocates through its allocator interface: an indirect call whose
    // first argument after `this` is the object size.
    let (allocation, size) = trace
        .calls
        .iter()
        .find_map(|call| match (call.target, call.args[1]) {
            (None, Some(Value::Imm(size))) => Some((call.ip, size)),
            _ => None,
        })
        .ok_or("allocator call with a constant size")?;
    let object = Value::Returned(allocation);
    if !trace
        .calls
        .iter()
        .any(|call| call.ip > allocation && call.target.is_some() && call.args[0] == Some(object))
    {
        return Err("constructor call on the allocation");
    }
    if trace.returned != Some(object) {
        return Err("allocation returned");
    }
    usize::try_from(size)
        .ok()
        .filter(|size| (MIN_OBJECT_SIZE..=MAX_OBJECT_SIZE).contains(size))
        .ok_or("plausible packet size")
}

/// Decode all three functions and require them to describe one object.
pub fn decode_api(
    bitness: u32,
    code: &[u8],
    alloc: usize,
    init: usize,
    release: usize,
) -> Result<PacketApi, &'static str> {
    decode_api_in(Abi::SysV, bitness, code, alloc, init, release)
}

/// [`decode_api`] for a named call convention.
pub fn decode_api_in(
    abi: Abi,
    bitness: u32,
    code: &[u8],
    alloc: usize,
    init: usize,
    release: usize,
) -> Result<PacketApi, &'static str> {
    let pointer_size = bitness as usize / 8;
    let fields = decode_init_in(abi, bitness, code, init)?;
    let refcount = decode_release_in(abi, bitness, code, release)?;
    let object_size = decode_alloc_in(abi, bitness, code, alloc)?;
    let mut spans = fields.spans(pointer_size).to_vec();
    spans.push((refcount, 4));
    if overlaps(&spans) {
        return Err("refcount apart from the packet fields");
    }
    if fields.end(pointer_size).max(refcount + 4) > object_size {
        return Err("packet fields inside the allocation");
    }
    Ok(PacketApi {
        fields,
        refcount,
        object_size,
    })
}

/// Decode the three functions out of one PE executable section.
///
/// Offsets are relative to the start of `segment`, which is what the scanner
/// reports, and the convention follows from the container: a 64-bit PE is
/// MS-x64, so its `Init` is `(this, conn_id, data, size, owned_data, add_ref)`
/// in `rcx, edx, r8, r9d` with the owned buffer pushed.
pub fn decode_api_segment(
    segment: &CodeSegment<'_>,
    alloc: usize,
    init: usize,
    release: usize,
) -> Result<PacketApi, &'static str> {
    decode_api_in(
        convention(segment),
        u32::from(segment.bits),
        segment.bytes,
        alloc,
        init,
        release,
    )
}

/// The call convention a segment's code was compiled for.
pub fn convention(segment: &CodeSegment<'_>) -> Abi {
    match segment.container {
        Container::Pe if segment.bits == 64 => Abi::MsX64,
        _ => Abi::SysV,
    }
}

/// The walk shape each function needs, for one convention.
fn alloc_walk(abi: Abi) -> Walk {
    match abi {
        Abi::SysV => Walk::linear(abi, ALLOC_WINDOW),
        Abi::MsX64 => Walk {
            follow_forward_jumps: true,
            ..Walk::linear(abi, ALLOC_WINDOW_MSX64)
        },
    }
}

fn overlaps(spans: &[(usize, usize)]) -> bool {
    spans.iter().enumerate().any(|(index, &(start, width))| {
        spans[index + 1..]
            .iter()
            .any(|&(other, other_width)| start < other + other_width && other < start + width)
    })
}

#[cfg(test)]
mod tests;
