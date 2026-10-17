use std::ops::Range;

use gpui_kit::{
    Context, FontWeight, HighlightStyle, SharedString, UnderlineStyle, Window,
    component::input::{
        EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
    },
    px,
};
use typst_syntax::{LinkedNode, SyntaxKind, Tag, highlight};

/// Adapt Typst's own parser to GPUI Kit's editor, rather than using an
/// approximate Markdown grammar for Typst code and equations.
#[derive(Default)]
pub struct TypstHighlighter {
    runs: Vec<(Range<usize>, Option<Tag>, bool)>,
}

fn collect(
    node: &LinkedNode<'_>,
    inherited: Option<Tag>,
    heading: bool,
    runs: &mut Vec<(Range<usize>, Option<Tag>, bool)>,
) {
    let tag = highlight(node).or(inherited);
    let heading = heading || node.kind() == SyntaxKind::Heading;
    if node.children().next().is_none() {
        if !node.range().is_empty() {
            runs.push((node.range(), tag, heading));
        }
    } else {
        for child in node.children() {
            collect(&child, tag, heading, runs);
        }
    }
}

impl TypstHighlighter {
    fn parse(&mut self, text: &str) {
        self.runs.clear();
        let root = typst_syntax::parse(text);
        collect(&LinkedNode::new(&root), None, false, &mut self.runs);
    }
}

impl InputHighlighter for TypstHighlighter {
    fn language(&self) -> SharedString {
        "typst".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.parse(&text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut result = Vec::new();
        let mut offset = range.start;
        for (span, tag, heading) in &self.runs {
            if span.end <= range.start {
                continue;
            }
            if span.start >= range.end {
                break;
            }
            let start = span.start.max(range.start);
            let end = span.end.min(range.end);
            if offset < start {
                result.push((offset..start, HighlightStyle::default()));
            }
            let name = match tag {
                Some(Tag::Comment) => "comment",
                Some(Tag::String | Tag::Raw) => "string",
                Some(Tag::Number) => "number",
                Some(Tag::Keyword) => "keyword",
                Some(Tag::Function) => "function",
                Some(Tag::Heading) => "title",
                Some(Tag::MathDelimiter | Tag::MathOperator | Tag::Operator | Tag::Escape) => {
                    "operator"
                }
                Some(Tag::Interpolated | Tag::Ref | Tag::Label | Tag::Link) => "type",
                Some(Tag::Punctuation | Tag::ListMarker) => "punctuation",
                _ => "",
            };
            let mut style = resolver.style(name).unwrap_or_default();
            // Heading is a structural property: keep the underline across
            // emphasized text, references, and interpolated code in its title.
            if *heading {
                style.color = Some(gpui_kit::rgb(0x000000).into());
                style.font_weight = Some(FontWeight::BOLD);
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                });
            }
            result.push((start..end, style));
            offset = end;
        }
        if offset < range.end {
            result.push((offset..range.end, HighlightStyle::default()));
        }
        result
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Palette;
    impl HighlightStyleResolver for Palette {
        fn style(&self, name: &str) -> Option<HighlightStyle> {
            Some(HighlightStyle {
                color: Some(if name == "number" {
                    gpui_kit::rgb(0xd04060).into()
                } else {
                    gpui_kit::rgb(0x111111).into()
                }),
                ..Default::default()
            })
        }
    }
    #[::core::prelude::v1::test]
    fn heading_underlines_cover_inline_markup_and_clear_after_edits() {
        let source = "= Introduction\nPlain text\n== In *this* paper #emph[é] 42\n#let text = \"= literal\"\n";
        let mut highlighter = TypstHighlighter::default();
        highlighter.parse(source);
        let styles = highlighter.styles(&(0..source.len()), &Palette);
        let second = source.find("==").unwrap();
        let end = source[second..].find('\n').unwrap() + second;
        for (at, ch) in source.char_indices() {
            if ch == '\n' {
                continue;
            }
            let (_, style) = styles
                .iter()
                .find(|(range, _)| range.contains(&at))
                .unwrap();
            let heading = at < "= Introduction".len() || (second..end).contains(&at);
            assert_eq!(style.underline.is_some(), heading, "at byte {at}: {ch}");
            if heading {
                assert_eq!(style.color, Some(gpui_kit::rgb(0x000000).into()));
                assert_eq!(style.font_weight, Some(FontWeight::BOLD));
            }
        }
        let plain = "Introduction\n#let n = 42";
        highlighter.parse(plain);
        let styles = highlighter.styles(&(0..plain.len()), &Palette);
        assert!(styles.iter().all(|(_, style)| style.underline.is_none()));
        let number = plain.find("42").unwrap();
        assert_eq!(
            styles
                .iter()
                .find(|(range, _)| range.contains(&number))
                .unwrap()
                .1
                .color,
            Some(gpui_kit::rgb(0xd04060).into())
        );
    }
}
