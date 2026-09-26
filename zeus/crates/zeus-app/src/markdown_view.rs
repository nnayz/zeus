//! Native GPUI presentation for the bounded Markdown document model.
//!
//! Parsing and sanitization live in `markdown`; this module translates the
//! resulting blocks into the inspector's visual language. Keeping the view
//! here gives PR descriptions, discussions, and future agent notes one
//! consistent readable measure and typographic hierarchy.

use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px, rgba};
use zeus_ui::{Ink, Radius, SemanticColors};

use crate::markdown::{InlineText, MarkdownBlock, MarkdownDocument};

/// Inspector notes stay on the compact secondary measure.
const INSPECTOR: MarkdownStyle = MarkdownStyle {
    max_width: 760.0,
    body: 11.5,
    body_line: 18.0,
    code: 10.5,
    code_line: 17.0,
    primary_body: false,
};

/// Zeron's transcript: 14/22 primary ink inside the conversation column.
const CHAT: MarkdownStyle = MarkdownStyle {
    max_width: 736.0,
    body: 14.0,
    body_line: 22.0,
    code: 13.0,
    code_line: 20.0,
    primary_body: true,
};

#[derive(Clone, Copy)]
struct MarkdownStyle {
    max_width: f32,
    body: f32,
    body_line: f32,
    code: f32,
    code_line: f32,
    primary_body: bool,
}

pub fn render_markdown(document: &MarkdownDocument, colors: SemanticColors) -> AnyElement {
    render_markdown_with(document, colors, INSPECTOR)
}

pub fn render_chat_markdown(document: &MarkdownDocument, colors: SemanticColors) -> AnyElement {
    render_markdown_with(document, colors, CHAT)
}

fn render_markdown_with(
    document: &MarkdownDocument,
    colors: SemanticColors,
    style: MarkdownStyle,
) -> AnyElement {
    let mut content = div()
        .w_full()
        .max_w(px(style.max_width))
        .flex()
        .flex_col()
        .gap(px(if style.primary_body { 12.0 } else { 10.0 }));

    for block in &document.blocks {
        content = content.child(render_block(block, colors, style));
    }

    content.into_any_element()
}

fn render_block(block: &MarkdownBlock, colors: SemanticColors, style: MarkdownStyle) -> AnyElement {
    let body_color = if style.primary_body {
        colors.primary
    } else {
        colors.secondary
    };
    match block {
        MarkdownBlock::Heading { level, content } => {
            let (size, line_height, top) = if style.primary_body {
                match *level {
                    1 => (20.0, 26.0, 6.0),
                    2 => (16.0, 22.0, 4.0),
                    _ => (15.0, 22.0, 2.0),
                }
            } else {
                match *level {
                    1 => (18.0, 23.0, 5.0),
                    2 => (15.0, 20.0, 4.0),
                    3 => (13.0, 18.0, 3.0),
                    _ => (12.0, 17.0, 2.0),
                }
            };
            div()
                .mt(px(top))
                .line_height(px(line_height))
                .text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.primary)
                .child(render_inline(content, colors, style, true))
                .into_any_element()
        }
        MarkdownBlock::Paragraph(content) => div()
            .line_height(px(style.body_line))
            .text_size(px(style.body))
            .text_color(body_color)
            .child(render_inline(content, colors, style, false))
            .into_any_element(),
        MarkdownBlock::List {
            ordered,
            start,
            items,
        } => {
            let mut list = div().flex().flex_col().gap(px(6.0));
            for (index, item) in items.iter().enumerate() {
                let marker = match item.checked {
                    Some(true) => "✓".to_owned(),
                    Some(false) => "○".to_owned(),
                    None if *ordered => format!("{}.", start + index),
                    None => "•".to_owned(),
                };
                let marker_color = match item.checked {
                    Some(true) => Ink::FRESH,
                    Some(false) => colors.tertiary,
                    None => colors.tertiary,
                };
                list = list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.0))
                        .line_height(px(style.body_line))
                        .text_size(px(style.body))
                        .text_color(body_color)
                        .child(
                            div()
                                .w(px(18.0))
                                .flex_none()
                                .text_right()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(marker_color)
                                .child(marker),
                        )
                        .child(div().min_w(px(0.0)).flex_1().child(render_inline(
                            &item.content,
                            colors,
                            style,
                            false,
                        ))),
                );
            }
            list.into_any_element()
        }
        MarkdownBlock::Quote(blocks) => {
            let mut quote = div()
                .pl(px(11.0))
                .py(px(3.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .border_l_2()
                .border_color(rgba(0xd9775788))
                .text_color(colors.secondary);
            for block in blocks {
                quote = quote.child(render_block(block, colors, style));
            }
            quote.into_any_element()
        }
        MarkdownBlock::CodeBlock { language, code } => {
            let mut code_lines = div()
                .w_full()
                .p(px(10.0))
                .flex()
                .flex_col()
                .font_family(crate::fonts::mono_family())
                .line_height(px(style.code_line))
                .text_size(px(style.code))
                .text_color(rgba(0xd8dee9ff));
            for line in code.lines() {
                code_lines = code_lines.child(
                    div()
                        .whitespace_nowrap()
                        .child(SharedString::from(if line.is_empty() { " " } else { line })),
                );
            }
            div()
                .rounded(px(Radius::BADGE))
                .bg(rgba(0x0d1117aa))
                .border_1()
                .border_color(colors.primary.alpha(0.075))
                .overflow_hidden()
                .when_some(language.clone(), |surface, language| {
                    surface.child(
                        div()
                            .h(px(25.0))
                            .px(px(9.0))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(colors.primary.alpha(0.06))
                            .text_size(px(9.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.tertiary)
                            .child(language),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("markdown-code-{code:p}")))
                        .w_full()
                        .overflow_x_scroll()
                        .child(code_lines),
                )
                .into_any_element()
        }
        MarkdownBlock::ThematicBreak => div()
            .my(px(3.0))
            .h(px(1.0))
            .w_full()
            .bg(colors.primary.alpha(0.08))
            .into_any_element(),
    }
}

fn render_inline(
    content: &InlineText,
    colors: SemanticColors,
    style: MarkdownStyle,
    heading: bool,
) -> AnyElement {
    let mut line = div().min_w(px(0.0)).flex().flex_wrap().items_baseline();
    for span in &content.spans {
        if span.text.is_empty() {
            continue;
        }
        let marks = span.style;
        let link = span.link.clone();
        let text = SharedString::from(span.text.clone());
        line = line.child(
            div()
                .id(SharedString::from(format!("markdown-inline-{span:p}")))
                .when(marks.bold, |piece| piece.font_weight(FontWeight::SEMIBOLD))
                .when(marks.italic, |piece| piece.italic())
                .when(marks.strikethrough, |piece| piece.line_through())
                .when(marks.code, |piece| {
                    piece
                        .mx(px(1.0))
                        .px(px(4.0))
                        .rounded(px(4.0))
                        .bg(colors.primary.alpha(0.065))
                        .font_family(crate::fonts::mono_family())
                        .text_size(px(if heading {
                            0.92 * style.body
                        } else {
                            style.code
                        }))
                        .text_color(rgba(0xe7b49fff))
                })
                .when(link.is_some(), |piece| {
                    piece
                        .underline()
                        .cursor_pointer()
                        .text_color(rgba(0x8bb9e8ff))
                })
                .when_some(link, |piece, link| {
                    piece.on_click(move |_, _, cx| {
                        cx.open_url(&link);
                        cx.stop_propagation();
                    })
                })
                .child(text),
        );
    }
    line.into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renderer_source_keeps_a_readable_measure_and_native_block_treatment() {
        let source = include_str!("markdown_view.rs");
        assert!(source.contains("max_width: 760.0"));
        assert!(source.contains("MarkdownBlock::CodeBlock"));
        assert!(source.contains("MarkdownBlock::List"));
        assert!(source.contains("MarkdownBlock::Quote"));
    }
}
