use super::*;

const COLS: u16 = 106;
const ROWS: u16 = 20;

fn bidi_config() -> Config {
    let mut config = Config::default();
    config.ui.pane_transforms = vec!["bidi".into()];
    config
}

/// One pane whose two rows hold `top` and `bottom`, each `width` cells wide.
fn text_surface(top: &str, bottom: &str, width: u16) -> PaneSurfaceFrame {
    let pad = |line: &str| {
        let mut line = line.to_owned();
        while line.chars().count() < usize::from(width) {
            line.push(' ');
        }
        line
    };
    let mut surface = surface();
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &Buffer::with_lines([pad(top), pad(bottom)]),
        None,
        &[],
    );
    for pane in &mut surface.panes {
        pane.rect.width = width;
        pane.inner_rect.width = width;
    }
    surface
}

fn shell(config: &Config, surface: PaneSurfaceFrame) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface);
    state.compose(COLS, ROWS).expect("pane frame");
    state
}

fn pane_row(frame: &FrameData, inner: Rect, row: u16) -> String {
    let start = usize::from(inner.y + row) * usize::from(frame.width) + usize::from(inner.x);
    frame.cells[start..start + usize::from(inner.width)]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect()
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

#[test]
fn bidi_reorders_pane_rows_and_leaves_everything_else_identical() {
    let off = shell(&Config::default(), text_surface("שלום ab", "PANE", 8))
        .compose(COLS, ROWS)
        .unwrap()
        .frame;
    let mut on_state = shell(&bidi_config(), text_surface("שלום ab", "PANE", 8));
    let on = on_state.compose(COLS, ROWS).unwrap().frame;
    let inner = on_state.hits.panes[0].inner_rect;

    assert_eq!(pane_row(&off, inner, 0), "שלום ab ");
    assert_eq!(pane_row(&on, inner, 0), "םולש ab ");
    assert_eq!(pane_row(&on, inner, 1), "PANE    ");
    for y in 0..on.height {
        for x in 0..on.width {
            let inside = y == inner.y && x >= inner.x && x < inner.x + inner.width;
            if !inside {
                let index = usize::from(y) * usize::from(on.width) + usize::from(x);
                assert_eq!(on.cells[index], off.cells[index], "cell ({x}, {y})");
            }
        }
    }
}

#[test]
fn default_config_keeps_logical_order() {
    let mut state = shell(&Config::default(), text_surface("שלום", "PANE", 4));
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    assert_eq!(pane_row(&frame, state.hits.panes[0].inner_rect, 0), "שלום");
    assert!(state.display_maps.is_empty());
}

#[test]
fn side_by_side_panes_reorder_independently() {
    let mut surface = text_surface("אב cd", "PANE ", 5);
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &Buffer::with_lines(["אב│גד", "PA│NE"]),
        None,
        &[],
    );
    surface.panes[0].rect.width = 2;
    surface.panes[0].inner_rect.width = 2;
    let mut right = surface.panes[0].clone();
    right.pane_id = "pane_2".into();
    right.rect.x = 3;
    right.inner_rect.x = 3;
    surface.panes.push(right);
    let mut state = shell(&bidi_config(), surface);
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    let left = state.hits.panes[0].inner_rect;
    let row: String = (left.x..left.x + 5)
        .map(|x| {
            frame.cells[usize::from(left.y) * usize::from(frame.width) + usize::from(x)]
                .symbol
                .clone()
        })
        .collect();
    assert_eq!(row, "בא│דג");
}

#[test]
fn cursor_follows_its_glyph_on_reordered_rows() {
    let mut surface = text_surface("אבגד", "PANE", 4);
    surface.frame.cursor = Some(crate::protocol::CursorState {
        x: 1,
        y: 0,
        visible: true,
        shape: 2,
    });
    let mut state = shell(&bidi_config(), surface);
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    let inner = state.hits.panes[0].inner_rect;
    assert_eq!(pane_row(&frame, inner, 0), "דגבא");
    let cursor = frame.cursor.expect("cursor");
    assert_eq!((cursor.x, cursor.y), (inner.x + 2, inner.y));

    let mut ascii = text_surface("abcd", "PANE", 4);
    ascii.frame.cursor = Some(crate::protocol::CursorState {
        x: 1,
        y: 0,
        visible: true,
        shape: 2,
    });
    let mut state = shell(&bidi_config(), ascii);
    let cursor = state.compose(COLS, ROWS).unwrap().frame.cursor.unwrap();
    assert_eq!(cursor.x, state.hits.panes[0].inner_rect.x + 1);
}

#[test]
fn mouse_selection_uses_logical_columns_and_highlights_the_selected_glyphs() {
    let mut state = shell(&bidi_config(), text_surface("אבגד", "PANE", 4));
    let inner = state.hits.panes[0].inner_rect;
    // Visual "דגבא": א is drawn at column 3 and ב at column 2.
    state.handle_raw_events(vec![RawInputEvent::Mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        inner.x + 3,
        inner.y,
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        inner.x + 2,
        inner.y,
    ))]);
    let selection = state.selection.as_ref().expect("selection");
    assert_eq!(selection.ordered_cells(), ((0, 0), (0, 1)));

    let frame = state.compose(COLS, ROWS).unwrap().frame;
    assert_eq!(pane_row(&frame, inner, 0), "דגבא");
    let bg = |col: u16| {
        frame.cells[usize::from(inner.y) * usize::from(frame.width) + usize::from(inner.x + col)].bg
    };
    assert_eq!(bg(2), bg(3), "selected glyphs share the highlight");
    assert_ne!(bg(0), bg(3), "unselected glyphs are not highlighted");
    assert_eq!(bg(0), bg(1));
}

#[test]
fn forwarded_mouse_reports_and_link_clicks_use_logical_columns() {
    let mut surface = text_surface("אבגד", "PANE", 4);
    surface.panes[0].mouse_reporting = true;
    let mut state = shell(&bidi_config(), surface);
    let inner = state.hits.panes[0].inner_rect;
    let down = state.handle_raw_events(vec![RawInputEvent::Mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        inner.x + 3,
        inner.y,
    ))]);
    assert!(matches!(
        &down.requests[..],
        [ClientMessage::ClientShellPaneInput { events, .. }]
            if matches!(
                &events[..],
                [ClientPaneInputEvent::Mouse {
                    position: ClientMousePosition::Cell { column: 0, row: 0 },
                    ..
                }]
            )
    ));

    let mut state = shell(&bidi_config(), text_surface("אבגד", "PANE", 4));
    let inner = state.hits.panes[0].inner_rect;
    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        modifiers: KeyModifiers::CONTROL,
        ..mouse(MouseEventKind::Down(MouseButton::Left), inner.x, inner.y)
    })]);
    assert!(matches!(
        &click.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneLinkActivate(params) if params.col == 3
            )
    ));
}

#[test]
fn sgr_pixel_reports_shift_with_the_column_remap() {
    let mut surface = text_surface("אבגד", "PANE", 4);
    surface.panes[0].mouse_reporting = true;
    surface.panes[0].sgr_pixel_mouse = true;
    surface.panes[0].pixel_width = 40;
    surface.panes[0].pixel_height = 40;
    let mut state = shell(&bidi_config(), surface);
    let inner = state.hits.panes[0].inner_rect;
    let geometry = crate::input::mouse::HostGeometry::new(COLS, ROWS, u32::from(COLS) * 10, 400)
        .expect("host geometry");
    // Pointer 3 px into visual column 3 (glyph א, logical column 0).
    let x = (u32::from(inner.x) + 3) * 10 + 3;
    let y = u32::from(inner.y) * 20 + 1;
    let down = state.handle_pixel_mouse(format!("\x1b[<0;{x};{y}M").as_bytes(), geometry);
    let [ClientMessage::ClientShellPaneInput { events, .. }] = &down.requests[..] else {
        panic!("expected one pane input request");
    };
    let [ClientPaneInputEvent::Mouse { position, .. }] = &events[..] else {
        panic!("expected one mouse event");
    };
    let ClientMousePosition::Pixels { x, column, .. } = position else {
        panic!("expected a pixel report, got {position:?}");
    };
    assert_eq!(*column, 0);
    assert_eq!(*x, 3);
}

fn patch(
    cells: Vec<crate::protocol::CellData>,
    x: u16,
    y: u16,
) -> crate::protocol::PaneSurfacePatch {
    let mut pane = surface().panes.remove(0);
    pane.rect.width = 4;
    pane.inner_rect.width = 4;
    pane.content_revision = 2;
    crate::protocol::PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        base_surface_revision: 1,
        surface_revision: 2,
        rows: vec![crate::protocol::PaneSurfacePatchRow { x, y, cells }],
        panes: vec![pane],
        cursor: None,
    }
}

fn cells(text: &str) -> Vec<crate::protocol::CellData> {
    FrameData::from_ratatui_buffer_with_hyperlinks(&Buffer::with_lines([text]), None, &[]).cells
}

#[test]
fn ascii_patches_on_left_to_right_rows_keep_the_fast_path() {
    let mut state = shell(&bidi_config(), text_surface("אבגד", "PANE", 4));
    assert!(matches!(
        state.apply_pane_surface_patch(patch(cells("x"), 0, 1)),
        ClientPaneSurfacePatchOutcome::Applied(Some(_))
    ));
}

#[test]
fn patches_on_right_to_left_rows_recompose() {
    // The old row holds Hebrew; an ASCII span must still recompose so the
    // whole row is laid out again ('א x' becomes 'א 1', drawn '1 א').
    let mut state = shell(&bidi_config(), text_surface("א x", "PANE", 4));
    assert!(matches!(
        state.apply_pane_surface_patch(patch(cells("1"), 2, 0)),
        ClientPaneSurfacePatchOutcome::Applied(None)
    ));
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    assert_eq!(pane_row(&frame, state.hits.panes[0].inner_rect, 0), "1 א ");

    // A Hebrew span arriving on an ASCII row recomposes too.
    let mut state = shell(&bidi_config(), text_surface("abcd", "PANE", 4));
    assert!(matches!(
        state.apply_pane_surface_patch(patch(cells("אב"), 0, 1)),
        ClientPaneSurfacePatchOutcome::Applied(None)
    ));

    // Without transforms the same patch keeps the fast path.
    let mut state = shell(&Config::default(), text_surface("א x", "PANE", 4));
    assert!(matches!(
        state.apply_pane_surface_patch(patch(cells("1"), 2, 0)),
        ClientPaneSurfacePatchOutcome::Applied(Some(_))
    ));
}

#[test]
fn popup_rows_reorder_inside_the_popup_only() {
    let mut surface = surface_with_popup();
    surface.popup.as_mut().unwrap().frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &Buffer::with_lines(["שלום      ", "", ""]),
        None,
        &[],
    );
    let mut state = shell(&bidi_config(), surface);
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    let popup = state.hits.popup.clone().expect("popup hit");
    let row = pane_row(&frame, popup.inner_rect, 0);
    assert!(row.starts_with("םולש"), "{row:?}");
    assert_eq!(state.popup_display_maps.len(), 1);
    assert!(state.display_maps.is_empty());
}

#[test]
fn live_reload_toggles_transforms_and_reports_unknown_names() {
    let mut state = shell(&Config::default(), text_surface("שלום", "PANE", 4));
    let mut config = bidi_config();
    config.ui.pane_transforms.push("rainbow".into());
    let diagnostics = state.config.apply_live_config(&config, &[], &[]);
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("unknown transform \"rainbow\"")));
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    assert_eq!(pane_row(&frame, state.hits.panes[0].inner_rect, 0), "םולש");

    state.config.apply_live_config(&Config::default(), &[], &[]);
    let frame = state.compose(COLS, ROWS).unwrap().frame;
    assert_eq!(pane_row(&frame, state.hits.panes[0].inner_rect, 0), "שלום");
}
