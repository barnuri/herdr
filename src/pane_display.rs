//! Display transforms for pane rows.
//!
//! A pane display transform rearranges or restyles the cells of one pane row
//! segment after the client has composed the row, including logical selection,
//! search, and link highlights, and before the frame reaches the outer
//! terminal. A segment never extends past the pane's inner rectangle, so
//! borders, scrollbars, the sidebar, and neighbouring panes stay in place.
//!
//! Transforms only change presentation. Terminal state, copied text, and
//! `herdr agent read` keep logical order. The registry is closed: config and
//! plugins select built-in transforms by name and never supply code, because
//! this runs on every composed frame for every visible pane row.

use crate::protocol::CellData;

pub(crate) mod bidi;
#[cfg(test)]
mod tests;

/// A built-in, in-process transform over one pane row segment.
///
/// A segment has one cell per column. A wide grapheme is a head cell followed
/// by spacer-tail cells whose symbol is empty; combining marks live inside the
/// head cell's symbol.
pub(crate) trait PaneRowTransform: Sync {
    /// Stable name used by `[ui] pane_transforms` and diagnostics.
    fn name(&self) -> &'static str;
    /// Cheap check that must not allocate. `false` means the transform would
    /// leave `segment` unchanged.
    fn affects(&self, segment: &[CellData]) -> bool;
    /// Rewrites `segment` in place without changing its width. Writes the
    /// visual-to-logical column map into `visual_to_logical`, one entry per
    /// cell. A head cell and its spacer tails stay contiguous and in order.
    /// Returns `false` when nothing changed.
    fn apply(
        &self,
        segment: &mut [CellData],
        visual_to_logical: &mut Vec<u16>,
        scratch: &mut Scratch,
    ) -> bool;
}

/// Built-in transforms, looked up by name.
static BUILTINS: &[&dyn PaneRowTransform] = &[&bidi::BidiAutoLtr];

pub(crate) fn lookup(name: &str) -> Option<&'static dyn PaneRowTransform> {
    BUILTINS
        .iter()
        .copied()
        .find(|transform| transform.name() == name)
}

/// Names accepted by `[ui] pane_transforms`.
pub(crate) fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|transform| transform.name())
}

/// Ordered transforms selected for pane rows.
#[derive(Clone, Default)]
pub(crate) struct PaneDisplayChain(Vec<&'static dyn PaneRowTransform>);

impl PaneDisplayChain {
    /// Builds a chain from configured names. Unknown names are returned so the
    /// caller can report them; duplicates are applied once.
    pub(crate) fn from_names(names: &[String]) -> (Self, Vec<String>) {
        let mut transforms: Vec<&'static dyn PaneRowTransform> = Vec::new();
        let mut unknown = Vec::new();
        for name in names {
            match lookup(name.trim()) {
                Some(transform) => {
                    if !transforms
                        .iter()
                        .any(|existing| existing.name() == transform.name())
                    {
                        transforms.push(transform);
                    }
                }
                None => unknown.push(name.clone()),
            }
        }
        (Self(transforms), unknown)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|transform| transform.name()).collect()
    }

    /// Whether any transform in the chain could change `segment`.
    pub(crate) fn affects(&self, segment: &[CellData]) -> bool {
        self.0.iter().any(|transform| transform.affects(segment))
    }

    /// Runs every affecting transform and composes their column maps.
    ///
    /// Returns the visual-to-logical map when cells moved. When the combined
    /// result is not a valid permutation that keeps wide graphemes intact,
    /// the segment is restored to logical order and `None` is returned.
    pub(crate) fn apply_segment(
        &self,
        segment: &mut [CellData],
        scratch: &mut Scratch,
    ) -> Option<Box<[u16]>> {
        let len = segment.len();
        if len == 0 || len > usize::from(u16::MAX) || !self.affects(segment) {
            return None;
        }
        scratch.original.clear();
        scratch.original.extend_from_slice(segment);
        let mut total: Vec<u16> = (0..len as u16).collect();
        let mut step = std::mem::take(&mut scratch.step);
        let mut changed = false;
        for transform in &self.0 {
            if !transform.affects(segment) {
                continue;
            }
            step.clear();
            if !transform.apply(segment, &mut step, scratch) {
                continue;
            }
            changed = true;
            if step.len() != len || step.iter().any(|&source| usize::from(source) >= len) {
                total.clear();
                break;
            }
            let previous = total.clone();
            for (target, &source) in total.iter_mut().zip(&step) {
                *target = previous[usize::from(source)];
            }
        }
        scratch.step = step;
        if !changed {
            return None;
        }
        if !valid_permutation(&scratch.original, &total) {
            crate::render_prof::event("pane_display.invalid_fallback");
            segment.clone_from_slice(&scratch.original);
            return None;
        }
        if total.iter().enumerate().all(|(v, &l)| usize::from(l) == v) {
            return None;
        }
        Some(total.into_boxed_slice())
    }
}

/// Checks that `visual_to_logical` is a bijection and that every spacer tail
/// is still drawn directly after the cell that precedes it in logical order.
fn valid_permutation(logical: &[CellData], visual_to_logical: &[u16]) -> bool {
    let len = logical.len();
    if visual_to_logical.len() != len {
        return false;
    }
    let mut seen = vec![false; len];
    for &source in visual_to_logical {
        let source = usize::from(source);
        if source >= len || std::mem::replace(&mut seen[source], true) {
            return false;
        }
    }
    visual_to_logical.iter().enumerate().all(|(v, &source)| {
        let source = usize::from(source);
        !logical[source].symbol.is_empty()
            || source == 0
            || (v > 0 && usize::from(visual_to_logical[v - 1]) + 1 == source)
    })
}

/// Column map for one reordered pane row segment, in screen coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowMap {
    pub(crate) x: u16,
    pub(crate) y: u16,
    visual_to_logical: Box<[u16]>,
    logical_to_visual: Box<[u16]>,
}

impl RowMap {
    pub(crate) fn new(x: u16, y: u16, visual_to_logical: Box<[u16]>) -> Self {
        let mut logical_to_visual = vec![0u16; visual_to_logical.len()];
        for (visual, &logical) in visual_to_logical.iter().enumerate() {
            if let Some(slot) = logical_to_visual.get_mut(usize::from(logical)) {
                *slot = visual as u16;
            }
        }
        Self {
            x,
            y,
            visual_to_logical,
            logical_to_visual: logical_to_visual.into_boxed_slice(),
        }
    }

    fn width(&self) -> u16 {
        self.visual_to_logical.len() as u16
    }

    pub(crate) fn contains(&self, x: u16, y: u16) -> bool {
        y == self.y && x >= self.x && x < self.x.saturating_add(self.width())
    }

    /// Logical screen column of the cell drawn at visual `screen_x`.
    pub(crate) fn to_logical(&self, screen_x: u16) -> u16 {
        self.map(screen_x, &self.visual_to_logical)
    }

    /// Visual screen column where logical `screen_x` is drawn.
    pub(crate) fn to_visual(&self, screen_x: u16) -> u16 {
        self.map(screen_x, &self.logical_to_visual)
    }

    fn map(&self, screen_x: u16, table: &[u16]) -> u16 {
        screen_x
            .checked_sub(self.x)
            .and_then(|offset| table.get(usize::from(offset)))
            .map_or(screen_x, |&mapped| self.x + mapped)
    }
}

/// Reusable buffers so composing a frame does not allocate per row.
#[derive(Default)]
pub(crate) struct Scratch {
    text: String,
    units: Vec<bidi::Unit>,
    levels: Vec<unicode_bidi::Level>,
    cells: Vec<CellData>,
    original: Vec<CellData>,
    step: Vec<u16>,
}
