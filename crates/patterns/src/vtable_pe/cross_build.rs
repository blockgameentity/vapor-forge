//! Slot-index stability across two builds of the same module.
//!
//! A vtable is an ordered array, so "the sibling is at slot 24" only transfers
//! to another build when the order survived the rebuild. It usually does, but
//! not always: `CUser`'s main vtable is 260 slots in build 1794536281 and 261 in
//! 1795048523, so at least one method was inserted and every later index shifted.
//! This module therefore *measures* the alignment instead of trusting the index:
//! per slot, it compares the structural fingerprint of the two candidate
//! functions and reports how many slots keep the same shape at the same index.
//!
//! Everything here is pure: callers supply slot RVAs and shape fingerprints for
//! both images, so the alignment rules are unit-testable without a PE.

use super::Table;

/// One slot, paired across builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotAlignment {
    pub slot: usize,
    pub a_rva: u32,
    pub b_rva: Option<u32>,
    /// Both builds have a shape fingerprint and they are equal.
    pub same_shape: bool,
}

/// Alignment of one table in the reference build with one candidate table in
/// the other build.
#[derive(Clone, Debug)]
pub struct TableMatch {
    pub b_base_rva: u32,
    pub b_len: usize,
    pub same_length: bool,
    pub slots: Vec<SlotAlignment>,
    /// Slots where both sides had a fingerprint.
    pub comparable: usize,
    /// Slots whose fingerprints agree at the same index.
    pub agreement: usize,
}

impl TableMatch {
    pub fn agreement_fraction(&self) -> f64 {
        if self.comparable == 0 {
            return 0.0;
        }
        self.agreement as f64 / self.comparable as f64
    }

    /// Half the slots keeping the same shape is the acceptance bar. It is a
    /// reporting threshold, not a proof: a rebuild that reorders the class would
    /// score near this by chance only for very small tables, so small tables are
    /// flagged separately by the caller.
    pub fn credible(&self) -> bool {
        self.same_length && self.agreement * 2 >= self.comparable && self.comparable >= 3
    }

    /// `(slot, a_rva, b_rva)` for slots that changed structure at the same index.
    pub fn disagreements(&self) -> impl Iterator<Item = &SlotAlignment> {
        self.slots
            .iter()
            .filter(|slot| slot.b_rva.is_some() && !slot.same_shape)
    }
}

/// A table in the other build, with the fingerprints a caller precomputed.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub base_rva: u32,
    pub rvas: Vec<u32>,
    pub shapes: Vec<Option<u64>>,
    pub class: Option<String>,
}

impl Candidate {
    pub fn from_table(table: &Table, shapes: Vec<Option<u64>>, class: Option<String>) -> Self {
        Self {
            base_rva: table.base_rva,
            rvas: table.entries.clone(),
            shapes,
            class,
        }
    }
}

/// Align a reference table with a candidate of the other build by index.
pub fn align(a: &Table, a_shapes: &[Option<u64>], b: &Candidate) -> TableMatch {
    let len = a.len().min(b.rvas.len());
    let mut slots = Vec::with_capacity(a.len());
    let (mut comparable, mut agreement) = (0usize, 0usize);
    for slot in 0..a.len() {
        let a_shape = a_shapes.get(slot).copied().flatten();
        let b_rva = b.rvas.get(slot).copied();
        let b_shape = b.shapes.get(slot).copied().flatten();
        let same_shape = match (a_shape, b_shape) {
            (Some(a_shape), Some(b_shape)) => {
                comparable += 1;
                if a_shape == b_shape {
                    agreement += 1;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        slots.push(SlotAlignment {
            slot,
            a_rva: a.entries[slot],
            b_rva: if slot < len { b_rva } else { None },
            same_shape,
        });
    }
    TableMatch {
        b_base_rva: b.base_rva,
        b_len: b.rvas.len(),
        same_length: a.len() == b.rvas.len(),
        slots,
        comparable,
        agreement,
    }
}

/// The candidate with the most shape agreement against `a`. Ties are broken by
/// the shorter table and then by the lower base RVA so the result is stable.
pub fn best_match(
    a: &Table,
    a_shapes: &[Option<u64>],
    candidates: &[Candidate],
) -> Option<TableMatch> {
    candidates
        .iter()
        .map(|candidate| align(a, a_shapes, candidate))
        .max_by(|left, right| {
            left.agreement
                .cmp(&right.agreement)
                .then(left.same_length.cmp(&right.same_length))
                .then(right.b_len.cmp(&left.b_len))
                .then(right.b_base_rva.cmp(&left.b_base_rva))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vtable_pe::Corroboration;

    fn table(base_rva: u32, entries: &[u32]) -> Table {
        Table {
            base_rva,
            section: ".rdata".to_owned(),
            entries: entries.to_vec(),
            corroboration: Corroboration::default(),
        }
    }

    #[test]
    fn identical_tables_agree_on_every_slot() {
        let a = table(0x2000, &[0x10, 0x20, 0x30]);
        let shapes = [Some(1), Some(2), Some(3)];
        let b = Candidate {
            base_rva: 0x9000,
            rvas: vec![0x1010, 0x1020, 0x1030],
            shapes: vec![Some(1), Some(2), Some(3)],
            class: Some("CUser".to_owned()),
        };
        let matched = align(&a, &shapes, &b);
        assert_eq!(matched.agreement, 3);
        assert_eq!(matched.comparable, 3);
        assert!(matched.credible());
        assert_eq!(matched.disagreements().count(), 0);
    }

    #[test]
    fn an_inserted_slot_is_visible_as_a_shift() {
        let a = table(0x2000, &[0x10, 0x20, 0x30]);
        let shapes = [Some(1), Some(2), Some(3)];
        // The other build inserted a method at slot 1, shifting the tail.
        let b = Candidate {
            base_rva: 0x9000,
            rvas: vec![0x1010, 0x9999, 0x1020, 0x1030],
            shapes: vec![Some(1), Some(9), Some(3), Some(2)],
            class: None,
        };
        let matched = align(&a, &shapes, &b);
        assert_eq!(matched.agreement, 2, "slots 0 and 2 keep their shape");
        assert!(!matched.same_length);
        assert!(!matched.credible());
        assert_eq!(matched.disagreements().count(), 1);
    }

    #[test]
    fn best_match_prefers_the_highest_agreement() {
        let a = table(0x2000, &[0x10, 0x20, 0x30]);
        let shapes = [Some(1), Some(2), Some(3)];
        let unrelated = Candidate {
            base_rva: 0x1000,
            rvas: vec![0x40, 0x50, 0x60],
            shapes: vec![Some(7), Some(8), Some(9)],
            class: None,
        };
        let related = Candidate {
            base_rva: 0x8000,
            rvas: vec![0x70, 0x80, 0x90],
            shapes: vec![Some(1), Some(2), Some(42)],
            class: None,
        };
        let matched = best_match(&a, &shapes, &[unrelated, related]).expect("match");
        assert_eq!(matched.b_base_rva, 0x8000);
        assert_eq!(matched.agreement, 2);
    }
}
