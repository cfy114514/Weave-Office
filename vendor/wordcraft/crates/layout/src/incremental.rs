//! Re-shape a locally edited paragraph when the existing pagination still fits.
use super::{Arc, DocLayout, Document, FieldCtx, LayoutCache, LayoutOptions, Path, Placed, StoryRef, ViewMode, cache, para, styleref};

#[derive(Clone, Debug)]
pub(crate) struct ParagraphMeasure {
    pub width: f32,
    pub grid: para::Grid,
    pub cell: Option<Arc<para::CellText>>,
}

impl DocLayout {
    /// The caller verified that only the text/runs at this path changed and that the previous
    /// layout has the same view settings. AutoFit, frames, split pieces, body page/column breaks
    /// and text-dependent headers need the full pagination pass when a paragraph changes.
    pub fn reflow_text(&self, doc: &Document, cache: &LayoutCache, opts: &LayoutOptions, path: &Path) -> Option<Self> {
        let started = super::now_ms();
        if opts.view != ViewMode::Print || opts.comment_balloons || cache.env != cache::env_hash(doc, opts) || styleref::headers_use_styleref(doc) {
            return None;
        }
        let story = StoryRef::Body;
        let p = doc.para(story, path)?;
        let [(pi, ii)] = self.index.get(&(story, path.clone()))?.as_slice() else { return None };
        let page = self.pages.get(*pi)?;
        let Placed::Lines { para: old, l0, l1, turn, .. } = page.items.get(*ii)? else { return None };
        let measure = old.reflow.as_ref()?;
        if *l0 != 0
            || *l1 != old.lines.len()
            || turn.is_turned()
            || old.rp.frame.is_some()
            || old.rp.drop_cap != 0
            || old.text_len == 0
            || p.text.is_empty()
            || !p.objects.is_empty()
            || p.section.is_some()
        {
            return None;
        }
        // Body pagination treats a break-only line and an empty mark after a page break
        // specially. Typing next to the break can change their page without changing any
        // line metrics. Cell paragraphs use box layout instead of those flow rules.
        if path.parent().is_empty() && old.lines.iter().any(|line| matches!(line.end, para::LineEnd::PageBreak | para::LineEnd::ColumnBreak)) {
            return None;
        }
        let mut blocks = &doc.body;
        for step in path.parent().chunks_exact(3) {
            let [b, r, c] = step else { return None };
            let table = blocks.get(*b as usize)?.as_table()?;
            let cell = table.rows.get(*r as usize)?.cells.get(*c as usize)?;
            if !table.props.fixed || cell.props.text_direction.is_turned() {
                return None;
            }
            blocks = &cell.blocks;
        }
        let fields = FieldCtx::default();
        let env = para::ParaEnv {
            doc,
            width: measure.width,
            grid: measure.grid,
            table: measure.cell.as_deref(),
            fields: &fields,
            label: None,
            show_hidden: opts.show_hidden,
            field_codes: opts.field_codes,
            hide_deleted: opts.hides_deleted(story),
            proofing: opts.proofing.then_some(opts.proof),
            typing_word: opts.typing_word.filter(|w| w.rev == p.rev).map(|w| (w.start, w.end)),
            numeral: opts.numeral,
            exclusions: &[],
            eq_number: 0,
            read_line_spacing: None,
            fit_objects: false,
            fitted: &[],
        };
        let mut new = para::layout_para(p, &env);
        // Widths within a line may change. Everything pagination, row splitting and
        // neighbouring blocks use to position text must still agree.
        let metrics = |l: &para::Line| (l.top, l.height, l.baseline, l.text_asc, l.text_desc, l.end, l.beside, l.left, l.right);
        if old.rp != new.rp
            || old.height != new.height
            || old.lines.len() != new.lines.len()
            || !old.lines.iter().zip(&new.lines).all(|(a, b)| metrics(a) == metrics(b))
        {
            return None;
        }
        new.reflow = Some(measure.clone());
        let mut updated = self.clone();
        let page = Arc::make_mut(updated.pages.get_mut(*pi)?);
        let Placed::Lines { para, .. } = page.items.get_mut(*ii)? else { return None };
        *para = Arc::new(new);
        updated.ms = super::now_ms() - started;
        Some(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_doc::{Block, Paragraph, Pos, Table, para_block};

    fn document(text: &str) -> Document {
        let mut doc = Document::from_text("after");
        doc.styles.default_chr.font = Some("Source Sans 3".into());
        doc.styles.default_chr.size = Some(12.0);
        doc.body.insert(0, para_block(Paragraph::with_text(text, Default::default())));
        doc
    }

    fn paragraph(layout: &DocLayout, path: &Path) -> Arc<para::ParaLayout> {
        let (page, item) = layout.index[&(StoryRef::Body, path.clone())][0];
        let Placed::Lines { para, .. } = &layout.pages[page].items[item] else { panic!("paragraph lines") };
        para.clone()
    }

    #[test]
    fn text_after_a_terminal_page_break_repagination() {
        let mut doc = document("before\u{000c}");
        let opts = LayoutOptions::default();
        let mut cache = LayoutCache::new();
        let old = crate::layout(&doc, &mut cache, &opts);
        let path = Path::top(0);
        assert_eq!(old.index[&(StoryRef::Body, path.clone())].len(), 1, "the trailing empty mark stays with its page break");
        let p = doc.para_mut(StoryRef::Body, &path).unwrap();
        p.insert_text(p.len(), "a", &Default::default()).unwrap();
        let fresh = crate::layout(&doc, &mut LayoutCache::new(), &opts);
        let (before, after) = (paragraph(&old, &path), paragraph(&fresh, &path));
        assert_eq!(before.lines.len(), after.lines.len(), "line count does not expose the new page boundary");
        assert_eq!(before.height, after.height, "paragraph height does not expose the new page boundary");
        assert_eq!(fresh.index[&(StoryRef::Body, path.clone())].len(), 2, "the new text follows the page break onto the next page");
        let end = Pos::body(0, doc.para(StoryRef::Body, &path).unwrap().len());
        assert_eq!(fresh.caret(&end).unwrap().page, 1);
        assert!(old.reflow_text(&doc, &cache, &opts, &path).is_none(), "hard-break edits require a pagination pass even at identical line heights");
    }

    #[test]
    fn ordinary_manual_line_breaks_keep_incremental_layout() {
        let mut doc = document("before\nafter");
        let opts = LayoutOptions::default();
        let mut cache = LayoutCache::new();
        let old = crate::layout(&doc, &mut cache, &opts);
        let path = Path::top(0);
        doc.para_mut(StoryRef::Body, &path).unwrap().insert_text(2, "a", &Default::default()).unwrap();
        let updated = old.reflow_text(&doc, &cache, &opts, &path).expect("manual line breaks do not force repagination");
        let fresh = crate::layout(&doc, &mut LayoutCache::new(), &opts);
        assert!(Arc::ptr_eq(&old.index, &updated.index));
        assert_eq!(updated.pages.len(), fresh.pages.len());
        for (block, item) in doc.body.iter().enumerate() {
            let Block::Para(p) = item.as_ref() else { panic!("body paragraph") };
            for offset in p.text.char_indices().map(|(offset, _)| offset).chain(std::iter::once(p.len())) {
                let at = Pos::body(block, offset);
                assert_eq!(updated.caret(&at), fresh.caret(&at), "caret at {at:?}");
            }
        }
    }

    #[test]
    fn cell_breaks_in_nested_fixed_tables_keep_incremental_layout() {
        for (text, lines) in [("before\u{000c}after", 1), ("before\u{000e}after", 2)] {
            let mut doc = document("unused");
            let mut inner = Table::new(1, 1, 200.0);
            inner.props.fixed = true;
            inner.rows[0].cells[0].blocks = vec![para_block(Paragraph::with_text(text, Default::default()))];
            let mut outer = Table::new(1, 1, 220.0);
            outer.props.fixed = true;
            outer.rows[0].cells[0].blocks = vec![Arc::new(Block::Table(inner))];
            doc.body[0] = Arc::new(Block::Table(outer));
            let path = Path(vec![0, 0, 0, 0, 0, 0, 0]);
            let opts = LayoutOptions::default();
            let mut cache = LayoutCache::new();
            let old = crate::layout(&doc, &mut cache, &opts);
            assert_eq!(paragraph(&old, &path).lines.len(), lines, "cell breaks are handled inside their row");
            doc.para_mut(StoryRef::Body, &path).unwrap().insert_text(2, "a", &Default::default()).unwrap();
            let updated = old.reflow_text(&doc, &cache, &opts, &path).expect("cell breaks in fixed tables remain safe");
            let fresh = crate::layout(&doc, &mut LayoutCache::new(), &opts);
            assert!(Arc::ptr_eq(&old.index, &updated.index));
            assert_eq!(updated.pages.len(), fresh.pages.len());
            let p = doc.para(StoryRef::Body, &path).unwrap();
            for offset in p.text.char_indices().map(|(offset, _)| offset).chain(std::iter::once(p.len())) {
                let at = Pos::new(StoryRef::Body, path.clone(), offset);
                assert_eq!(updated.caret(&at), fresh.caret(&at), "caret at {at:?}");
            }
            let cell_rects = |layout: &DocLayout| {
                layout
                    .pages
                    .iter()
                    .enumerate()
                    .flat_map(|(page, p)| {
                        p.items.iter().filter_map(move |item| match item {
                            Placed::Cell { rect, .. } => Some((page, *rect)),
                            _ => None,
                        })
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(cell_rects(&updated), cell_rects(&fresh));
        }
    }
}
