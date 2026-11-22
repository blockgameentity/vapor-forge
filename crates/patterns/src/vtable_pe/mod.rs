//! PE-side vtable enumeration.
//!
//! On Windows the four Gate-0 targets have no published signature hint, so the
//! next lever is *class structure*: a class's virtual methods are consecutive
//! function pointers in a read-only section, in a fixed order per compiler.
//! Finding the table that holds an anchored sibling gives the class, and the
//! sibling's slot index pins where the missing method sits in that order.
//!
//! What this module does, and nothing more:
//!
//! 1. walks every non-executable section as 8-byte-aligned qwords;
//! 2. keeps maximal runs whose qwords are all `image_base + rva` with `rva`
//!    inside an executable section, i.e. candidate vtables;
//! 3. rejects runs that are too short (a stray pointer) or too long (a
//!    function-pointer array, not a vtable);
//! 4. corroborates each accepted table without requiring it: how many slots are
//!    `.pdata` function entries, whether the qword before the table points at
//!    data (the MSVC RTTI locator) and, when it does, the RTTI class name.
//!
//! Nothing here executes or trusts the image, and a table is a *candidate*: the
//! slot order of a class is only stable across builds as far as the compiler
//! kept it, which [`cross_build`] measures instead of assuming.

use crate::pe_image::PeImage;

pub mod cross_build;
pub mod rtti;
pub mod scan;
pub mod structure;

pub use scan::enumerate;

/// A run of fewer consecutive function pointers than this is not a table.
pub const MIN_SLOTS: usize = 2;
/// Upper bound on a table. The longest plausible vtable measured in either
/// `steamclient64.dll` build is 261 slots (`CUser`); the longest qualifying run
/// is 5,054 pointers, which is a function-pointer array rather than a class.
pub const MAX_SLOTS: usize = 512;

/// Why a candidate run was not accepted as a table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    TooFewSlots {
        found: usize,
    },
    TooManySlots {
        found: usize,
    },
    /// Entry `slot` is not `image_base + <rva inside .text>`. Reachable through
    /// [`validate`]; the scanner itself stops a run at such a qword, which is
    /// what bounds a table.
    NonTextPointer {
        slot: usize,
    },
}

impl Rejection {
    pub fn summary(self) -> String {
        match self {
            Self::TooFewSlots { found } => format!("too few slots ({found})"),
            Self::TooManySlots { found } => format!("too many slots ({found})"),
            Self::NonTextPointer { slot } => format!("slot {slot} is not a .text pointer"),
        }
    }
}

/// Structural corroboration collected for an accepted table.
#[derive(Clone, Debug, Default)]
pub struct Corroboration {
    pub pdata_entries: usize,
    pub preceded_by_data_pointer: bool,
    pub rtti_class: Option<String>,
}

impl Corroboration {
    /// Fraction of slots that are `.pdata` function entries. Import thunks and
    /// `_purecall` stubs are inside `.text` but not in `.pdata`, so this is
    /// evidence, not a rule.
    pub fn pdata_coverage(&self, slots: usize) -> f64 {
        if slots == 0 {
            return 0.0;
        }
        self.pdata_entries as f64 / slots as f64
    }

    pub fn class_short_name(&self) -> Option<&str> {
        self.rtti_class.as_deref().map(rtti::short_name)
    }
}

/// A candidate vtable: consecutive function pointers in one data section.
#[derive(Clone, Debug)]
pub struct Table {
    pub base_rva: u32,
    /// Section the table lives in (`.rdata`, `.data`, ...).
    pub section: String,
    /// Function RVAs, slot 0 first.
    pub entries: Vec<u32>,
    pub corroboration: Corroboration,
}

impl Table {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn end_rva(&self) -> u32 {
        self.base_rva + (self.entries.len() as u32) * 8
    }

    pub fn slot_of(&self, rva: u32) -> Option<usize> {
        self.entries.iter().position(|entry| *entry == rva)
    }

    pub fn class(&self) -> Option<&str> {
        self.corroboration.class_short_name()
    }
}

/// One accepted table or one rejected run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejected {
    pub base_rva: u32,
    pub reason: Rejection,
}

#[derive(Clone, Debug, Default)]
pub struct Enumeration {
    pub tables: Vec<Table>,
    pub rejected: Vec<Rejected>,
}

impl Enumeration {
    pub fn with_class(&self) -> usize {
        self.tables
            .iter()
            .filter(|table| table.corroboration.rtti_class.is_some())
            .count()
    }
}

/// A table is a bounded run of consecutive pointers into executable code, with
/// a plausible slot count. `entries` are RVAs, not VAs.
pub fn validate(image: &PeImage<'_>, entries: &[u32]) -> Result<(), Rejection> {
    if entries.len() < MIN_SLOTS {
        return Err(Rejection::TooFewSlots {
            found: entries.len(),
        });
    }
    if entries.len() > MAX_SLOTS {
        return Err(Rejection::TooManySlots {
            found: entries.len(),
        });
    }
    for (slot, rva) in entries.iter().enumerate() {
        if !in_executable(image, u64::from(*rva)) {
            return Err(Rejection::NonTextPointer { slot });
        }
    }
    Ok(())
}

/// Tables holding `rva` in any slot.
pub fn containing(tables: &[Table], rva: u32) -> Vec<&Table> {
    tables
        .iter()
        .filter(|table| table.slot_of(rva).is_some())
        .collect()
}

/// Tables whose RTTI class matches `needle`: an exact case-insensitive match
/// wins, otherwise a substring match is used. The preference matters because
/// `CUser` is also a prefix of hundreds of `CUser_..._Request` protobuf message
/// classes, which are not the class under investigation.
pub fn of_class<'t>(tables: &'t [Table], needle: &str) -> Vec<&'t Table> {
    let exact = tables
        .iter()
        .filter(|table| {
            table
                .class()
                .is_some_and(|class| class.eq_ignore_ascii_case(needle))
        })
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    let needle = needle.to_ascii_lowercase();
    tables
        .iter()
        .filter(|table| {
            table
                .class()
                .is_some_and(|class| class.to_ascii_lowercase().contains(&needle))
        })
        .collect()
}

/// `value` -> RVA when it is `image_base + rva` with `rva` inside `.text`.
pub fn text_rva(image: &PeImage<'_>, value: u64) -> Option<u32> {
    let rva = value.checked_sub(image.image_base())?;
    let rva = u32::try_from(rva).ok()?;
    if in_executable(image, u64::from(rva)) {
        Some(rva)
    } else {
        None
    }
}

fn in_executable(image: &PeImage<'_>, rva: u64) -> bool {
    let Ok(rva) = u32::try_from(rva) else {
        return false;
    };
    image.sections().iter().any(|section| {
        section.executable
            && rva >= section.virtual_address
            && rva < section.virtual_address.saturating_add(section.raw_size)
    })
}

#[cfg(test)]
mod tests;
