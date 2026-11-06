//! Private placement of indivisible original register ranges.
use std::{collections::BTreeMap, ops::Range};

use rumoca_ir_solve::{LinearOp, Reg, RegisterVersionProgram};

#[derive(Clone, Debug)]
struct Component {
    virtual_range: Range<usize>,
    first: usize,
    last: usize,
    physical: usize,
}

#[derive(Debug)]
pub(in crate::emit) struct RegisterLayout {
    components: Vec<Component>,
    extent: usize,
}

impl RegisterLayout {
    pub(in crate::emit) fn derive(program: &[LinearOp]) -> Result<Self, String> {
        let versions = derive_versions(program)
            .ok_or("native register layout requires canonical register versions")?;
        let mut constraints = Vec::new();
        for (index, operation) in versions.operations().iter().enumerate() {
            if let Some((start, storage)) = operation.destination() {
                let start = start as usize;
                constraints.push(component(start..start + storage.len(), index, index));
            }
            let mut selections: BTreeMap<_, (Range<usize>, usize)> = BTreeMap::new();
            for piece in operation.operands() {
                let range = piece.registers();
                selections
                    .entry(piece.selection())
                    .and_modify(|entry| extend_selection(entry, &range, piece.producer()))
                    .or_insert((range, piece.producer()));
            }
            constraints.extend(
                selections
                    .into_values()
                    .map(|(range, first)| component(range, first, index)),
            );
        }
        constraints.retain(|component| !component.virtual_range.is_empty());
        constraints
            .sort_by_key(|component| (component.virtual_range.start, component.virtual_range.end));
        let mut components: Vec<Component> = Vec::new();
        for constraint in constraints {
            if let Some(previous) = components.last_mut()
                && constraint.virtual_range.start < previous.virtual_range.end
            {
                previous.virtual_range.end =
                    previous.virtual_range.end.max(constraint.virtual_range.end);
                previous.first = previous.first.min(constraint.first);
                previous.last = previous.last.max(constraint.last);
            } else {
                components.push(constraint);
            }
        }
        let extent = allocate(&mut components)?;
        Ok(Self { components, extent })
    }

    pub(in crate::emit) fn extent(&self) -> usize {
        self.extent
    }

    pub(in crate::emit) fn virtual_extent(&self) -> usize {
        self.components
            .last()
            .map_or(0, |component| component.virtual_range.end)
    }

    pub(in crate::emit) fn physical(&self, register: Reg) -> Result<Reg, String> {
        let register = register as usize;
        let index = self
            .components
            .partition_point(|component| component.virtual_range.start <= register);
        let component = index
            .checked_sub(1)
            .and_then(|index| self.components.get(index))
            .filter(|component| component.virtual_range.contains(&register))
            .ok_or("native register has no original placement")?;
        component
            .physical
            .checked_add(register - component.virtual_range.start)
            .and_then(|physical| Reg::try_from(physical).ok())
            .ok_or_else(|| "native physical register overflows".into())
    }
}

#[inline(never)]
fn derive_versions(program: &[LinearOp]) -> Option<RegisterVersionProgram> {
    RegisterVersionProgram::derive(program)
}

fn extend_selection(entry: &mut (Range<usize>, usize), range: &Range<usize>, producer: usize) {
    entry.0.start = entry.0.start.min(range.start);
    entry.0.end = entry.0.end.max(range.end);
    entry.1 = entry.1.min(producer);
}

fn component(virtual_range: Range<usize>, first: usize, last: usize) -> Component {
    Component {
        virtual_range,
        first,
        last,
        physical: 0,
    }
}

#[inline(never)]
fn allocate(components: &mut [Component]) -> Result<usize, String> {
    let mut order: Vec<_> = (0..components.len()).collect();
    order.sort_by_key(|&index| {
        let component = &components[index];
        (
            std::cmp::Reverse(component.virtual_range.len()),
            std::cmp::Reverse(component.last - component.first),
            component.virtual_range.start,
        )
    });
    // Placed components are kept by the blocks of operations their lifetimes touch, so a component
    // meets only the placed ones whose lifetimes share a block with its own instead of every one
    // (a straight-line program places thousands of short lifetimes, and the scan of all of them
    // for each made placement grow with the square of the program). The components found are
    // exactly those whose lifetimes overlap, so the placement is the one a full scan gives.
    const BLOCK: usize = 32;
    let horizon = components
        .iter()
        .map(|component| component.first.max(component.last))
        .max()
        .map_or(0, |last| last + 1);
    // A lifetime spanning more than LONG blocks is kept apart, sorted by its physical start: a
    // program holds thousands of them, and filing each in every block it touches made every short
    // lifetime scan and sort them again. They are met once per component, already in address order.
    const LONG: usize = 4;
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); horizon / BLOCK + 1];
    // (physical start, physical end, first, last) of the placed long lifetimes.
    let mut long: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut seen = vec![usize::MAX; components.len()];
    let mut extent = 0usize;
    for (turn, index) in order.into_iter().enumerate() {
        let (first, last, width) = {
            let current = &components[index];
            (current.first, current.last, current.virtual_range.len())
        };
        let blocks = first.min(last) / BLOCK..=first.max(last) / BLOCK;
        let mut local = Vec::new();
        for block in blocks.clone() {
            for &other in &buckets[block] {
                if seen[other] == turn {
                    continue;
                }
                seen[other] = turn;
                let other = &components[other];
                if other.first <= last && first <= other.last {
                    let end = other
                        .physical
                        .checked_add(other.virtual_range.len())
                        .ok_or("native register placement overflows")?;
                    local.push(other.physical..end);
                }
            }
        }
        local.sort_by_key(|range| (range.start, range.end));
        let mut local = local.into_iter().peekable();
        let mut long_iter = long
            .iter()
            .filter(|&&(_, _, first_other, last_other)| first_other <= last && first <= last_other)
            .map(|&(start, end, _, _)| start..end)
            .peekable();
        let mut physical = 0usize;
        loop {
            let range = match (local.peek(), long_iter.peek()) {
                (Some(a), Some(b)) if (b.start, b.end) < (a.start, a.end) => long_iter.next(),
                (Some(_), _) => local.next(),
                (None, Some(_)) => long_iter.next(),
                (None, None) => None,
            };
            let Some(range) = range else { break };
            let end = physical
                .checked_add(width)
                .ok_or("native register placement overflows")?;
            if end <= range.start {
                break;
            }
            physical = physical.max(range.end);
        }
        let end = physical
            .checked_add(width)
            .ok_or("native register placement overflows")?;
        components[index].physical = physical;
        extent = extent.max(end);
        if blocks.clone().count() > LONG {
            let key = (physical, end, first, last);
            let at = long.partition_point(|entry| (entry.0, entry.1) < (key.0, key.1));
            long.insert(at, key);
        } else {
            for block in blocks {
                buckets[block].push(index);
            }
        }
    }
    Ok(extent)
}

#[cfg(test)]
mod tests;
