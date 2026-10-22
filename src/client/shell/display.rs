//! Applies pane display transforms to composed frames and maps pane-content
//! columns from the drawn (visual) order back to logical columns.

use super::*;
use crate::pane_display::{PaneDisplayChain, RowMap, Scratch};

/// Transforms each row of the pane drawn in `inner`, in screen coordinates.
///
/// The `skip_row` row is left alone, so the mode bar stays intact.
/// The frame cursor follows its glyph when it sits on a reordered row.
pub(super) fn apply_pane_display(
    chain: &PaneDisplayChain,
    scratch: &mut Scratch,
    maps: &mut Vec<RowMap>,
    frame: &mut FrameData,
    inner: Rect,
    skip_row: Option<u16>,
) {
    if chain.is_empty() {
        return;
    }
    let width = usize::from(frame.width);
    let right = inner.x.saturating_add(inner.width).min(frame.width);
    if inner.x >= right {
        return;
    }
    let bottom = inner.y.saturating_add(inner.height).min(frame.height);
    for y in inner.y..bottom {
        if skip_row == Some(y) {
            continue;
        }
        crate::render_prof::counter("pane_display.rows_scanned", 1);
        let start = usize::from(y) * width + usize::from(inner.x);
        let end = usize::from(y) * width + usize::from(right);
        let Some(segment) = frame.cells.get_mut(start..end) else {
            continue;
        };
        let Some(visual_to_logical) = chain.apply_segment(segment, scratch) else {
            continue;
        };
        crate::render_prof::counter("pane_display.rows_reordered", 1);
        let map = RowMap::new(inner.x, y, visual_to_logical);
        if let Some(cursor) = frame
            .cursor
            .as_mut()
            .filter(|cursor| map.contains(cursor.x, cursor.y))
        {
            cursor.x = map.to_visual(cursor.x);
        }
        maps.push(map);
    }
}

/// Whether any surface graphics placement overlaps `inner`, given the pane
/// surface's origin on screen. Image cells are never reordered.
pub(super) fn graphics_overlap(
    graphics: &crate::protocol::SurfaceGraphicsScene,
    surface_origin: Rect,
    inner: Rect,
) -> bool {
    graphics.placements.iter().any(|placement| {
        let x = u32::from(surface_origin.x) + u32::from(placement.x);
        let y = u32::from(surface_origin.y) + u32::from(placement.y);
        let right = x.saturating_add(placement.cols);
        let bottom = y.saturating_add(placement.rows);
        x < u32::from(inner.x) + u32::from(inner.width)
            && right > u32::from(inner.x)
            && y < u32::from(inner.y) + u32::from(inner.height)
            && bottom > u32::from(inner.y)
    })
}

/// Shifts a pane-relative pixel x by `column_shift` cells, keeping the
/// pointer's offset inside its cell.
pub(super) fn shift_pixel_x(x: u32, hit: &PaneHit, column_shift: i32) -> u32 {
    if column_shift == 0 || hit.inner_rect.width == 0 {
        return x;
    }
    let cell_width = i64::from(hit.pixel_width / u32::from(hit.inner_rect.width));
    let shifted = i64::from(x) + i64::from(column_shift) * cell_width;
    shifted.clamp(0, i64::from(hit.pixel_width.saturating_sub(1))) as u32
}

impl ClientShellState {
    /// Logical screen column of the pane glyph drawn at (`column`, `row`).
    ///
    /// Only pane-content handling (forwarded mouse reports, selection, link
    /// hover and activation) converts columns through this. Shell chrome,
    /// menus, and overlays hit-test the raw pointer position. Popup maps win
    /// while the pointer is inside the popup.
    pub(super) fn content_column(&self, column: u16, row: u16) -> u16 {
        let in_popup = self
            .hits
            .popup
            .as_ref()
            .is_some_and(|hit| super::contains(hit.rect, (column, row)));
        let maps = if in_popup {
            &self.popup_display_maps
        } else {
            &self.display_maps
        };
        maps.iter()
            .find(|map| map.contains(column, row))
            .map_or(column, |map| map.to_logical(column))
    }

    /// Whether a surface patch could change a row the display transforms
    /// reorder. Such patches recompose instead of taking the span fast path,
    /// because the forwarded span would land on logical columns.
    pub(super) fn patch_touches_display_rows(
        &self,
        patch: &crate::protocol::PaneSurfacePatch,
    ) -> bool {
        let chain = &self.config.pane_display;
        if chain.is_empty() {
            return false;
        }
        let Some(surface) = self.pane_surface.as_ref() else {
            return true;
        };
        let frame = &surface.frame;
        let surface_row = |y: u16| {
            let width = usize::from(frame.width);
            let start = usize::from(y) * width;
            frame.cells.get(start..start + width).unwrap_or(&[])
        };
        patch
            .rows
            .iter()
            .any(|row| chain.affects(surface_row(row.y)) || chain.affects(&row.cells))
            || [frame.cursor.as_ref(), patch.cursor.as_ref()]
                .into_iter()
                .flatten()
                .any(|cursor| chain.affects(surface_row(cursor.y)))
    }
}
