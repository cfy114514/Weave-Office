use super::*;
use std::sync::Arc;
use wordcraft_doc::{Block, Cell, Paragraph, PartKind, Table, para_block};

fn lay(doc: &Document) -> DocLayout {
    crate::layout(doc, &mut crate::LayoutCache::new(), &crate::LayoutOptions::default())
}

// Page traversal and document traversal can visit repeated table headers in different orders.
// Compare a multiset so duplicate or missing rectangles still fail the assertion.
fn sorted(rects: Vec<(usize, Rect)>) -> Vec<(usize, [u32; 4])> {
    let mut out: Vec<_> = rects.into_iter().map(|(page, r)| (page, [r.x, r.y, r.w, r.h].map(f32::to_bits))).collect();
    out.sort_unstable();
    out
}

fn assert_viewports(doc: &Document, layout: &DocLayout, a: &Pos, b: &Pos, hint: usize) {
    let full = layout.selection_rects(doc, a, b, hint);
    assert!(!full.is_empty(), "the fixture must select laid-out text");
    for page in 0..=layout.pages.len() {
        let expected = sorted(full.iter().copied().filter(|(pi, _)| *pi == page).collect());
        for (a, b) in [(a, b), (b, a)] {
            assert_eq!(sorted(layout.selection_rects_on_pages(doc, a, b, hint, page..page + 1)), expected, "page {page}, hint {hint}");
        }
    }
    assert!(layout.selection_rects_on_pages(doc, a, b, hint, 1..1).is_empty());
    assert!(layout.selection_rects_on_pages(doc, a, b, hint, usize::MAX..usize::MAX).is_empty());
}

#[test]
fn visible_selection_keeps_split_paragraph_boundaries() {
    let mut doc = Document::from_text(&"one two three four five six seven eight nine ten ".repeat(80));
    doc.last_section.page_h = 240.0;
    doc.last_section.margin_top = 24.0;
    doc.last_section.margin_bottom = 24.0;
    let layout = lay(&doc);
    assert!(layout.pages.len() > 2);
    let next_page = layout.pages[1]
        .items
        .iter()
        .find_map(|item| match item {
            Placed::Lines { para, l0, .. } => Some(para.lines[*l0].start),
            _ => None,
        })
        .unwrap();
    assert!(next_page > 0);
    let end = doc.end_of(StoryRef::Body);
    // Both ends can share one paragraph even though the highlight spans several pages;
    // an end exactly on the following page's first line must not consume that line.
    assert_viewports(&doc, &layout, &Pos::body(0, 4), &Pos::body(0, next_page), 0);
    assert_viewports(&doc, &layout, &Pos::body(0, next_page), &end, 1);
}

#[test]
fn visible_selection_keeps_nested_cells_and_repeated_table_headings() {
    let mut doc = Document::from_text("before\nafter");
    let mut table = Table::new(24, 2, 360.0);
    table.props.fixed = true;
    table.rows[0].props.header = true;
    for (row, r) in table.rows.iter_mut().enumerate() {
        for (column, cell) in r.cells.iter_mut().enumerate() {
            *cell = Cell::with_text(&format!("Row {row} column {column} abc אבג"));
        }
    }
    let mut nested = Table::new(2, 1, 160.0);
    nested.rows[0].cells[0] = Cell::with_text("nested first");
    nested.rows[1].cells[0] = Cell::with_text("nested second");
    table.rows[2].cells[0].blocks.push(Arc::new(Block::Table(nested)));
    doc.body.insert(1, Arc::new(Block::Table(table)));
    doc.last_section.page_h = 260.0;
    doc.last_section.margin_top = 24.0;
    doc.last_section.margin_bottom = 24.0;
    let layout = lay(&doc);
    let heading = Path(vec![1, 0, 0, 0]);
    assert!(layout.index[&(StoryRef::Body, heading.clone())].len() > 1, "the heading must actually repeat");
    let a = doc.start_of(StoryRef::Body);
    let b = doc.end_of(StoryRef::Body);
    assert_viewports(&doc, &layout, &a, &b, 0);
    let nested_start = Pos::new(StoryRef::Body, Path(vec![1, 2, 0, 1, 0, 0, 0]), 2);
    assert_viewports(&doc, &layout, &nested_start, &b, 0);
    let column = layout.column_segments(&doc, ColumnSelection { start: &a, end: &b, left: 96.0, right: 116.0, page_hint: 0, max_lines: 500 });
    assert!(!column.is_empty());
    let full: Vec<_> = column.iter().flat_map(|(a, b)| layout.selection_rects(&doc, a, b, 0)).collect();
    for page in 0..layout.pages.len() {
        let expected = sorted(full.iter().copied().filter(|(pi, _)| *pi == page).collect());
        assert_eq!(sorted(layout.column_rects_on_pages(&doc, &column, 0, page..page + 1)), expected, "page {page}");
    }
}

#[test]
fn visible_header_selection_preserves_page_hints() {
    let mut doc = Document::from_text(&"body\n".repeat(80));
    let header = doc.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Header text", Default::default()))]);
    doc.last_section.headers.default = Some(header);
    let layout = lay(&doc);
    assert!(layout.pages.len() > 1);
    let (a, b) = (Pos::new(StoryRef::Part(header), Path::top(0), 1), Pos::new(StoryRef::Part(header), Path::top(0), 7));
    for hint in [0, layout.pages.len() - 1, usize::MAX] {
        assert_viewports(&doc, &layout, &a, &b, hint);
        let expected = if hint < layout.pages.len() { hint } else { 0 };
        assert!(layout.selection_rects(&doc, &a, &b, hint).iter().all(|(page, _)| *page == expected));
    }
}
