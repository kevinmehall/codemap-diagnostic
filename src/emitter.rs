// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::io::prelude::*;
use std::io;
use std::cmp::min;
use std::sync::Arc;
use std::collections::HashMap;
use term;
use isatty::stderr_isatty;

use { Emitter, Level, Diagnostic, SpanLabel, SpanStyle };
use codemap::{CodeMap, File};
use snippet::{Annotation, AnnotationType, Line, MultilineAnnotation, StyledString, Style};
use styled_buffer::StyledBuffer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorConfig {
    Auto,
    Always,
    Never,
}

impl ColorConfig {
    fn use_color(&self) -> bool {
        match *self {
            ColorConfig::Always => true,
            ColorConfig::Never => false,
            ColorConfig::Auto => stderr_isatty(),
        }
    }
}

pub struct EmitterWriter<'a> {
    dst: Destination,
    cm: Option<&'a CodeMap>,
}

impl<'a> Emitter for EmitterWriter<'a> {
    fn emit(&mut self, msgs: &[Diagnostic]) {
        self.emit_messages_default(msgs)
    }
}

struct FileWithAnnotatedLines {
    file: Arc<File>,
    lines: Vec<Line>,
    multiline_depth: usize,
}

impl<'a> EmitterWriter<'a> {
    pub fn stderr(color_config: ColorConfig, code_map: Option<&'a CodeMap>) -> EmitterWriter {
        if color_config.use_color() {
            let dst = Destination::from_stderr();
            EmitterWriter {
                dst: dst,
                cm: code_map,
            }
        } else {
            EmitterWriter {
                dst: Raw(Box::new(io::stderr())),
                cm: code_map,
            }
        }
    }

    pub fn new(dst: Box<Write + Send>, code_map: Option<&'a CodeMap>) -> EmitterWriter<'a> {
        EmitterWriter {
            dst: Raw(dst),
            cm: code_map,
        }
    }

    fn preprocess_annotations(cm: Option<&'a CodeMap>, spans: &[SpanLabel]) -> Vec<FileWithAnnotatedLines> {
        fn add_annotation_to_file<'a>(file_vec: &mut Vec<FileWithAnnotatedLines>,
                                  file: Arc<File>,
                                  line_index: usize,
                                  ann: Annotation) {

            for slot in file_vec.iter_mut() {
                // Look through each of our files for the one we're adding to
                if slot.file.name() == file.name() {
                    // See if we already have a line for it
                    for line_slot in &mut slot.lines {
                        if line_slot.line_index == line_index {
                            line_slot.annotations.push(ann);
                            return;
                        }
                    }
                    // We don't have a line yet, create one
                    slot.lines.push(Line {
                        line_index: line_index,
                        annotations: vec![ann],
                    });
                    slot.lines.sort();
                    return;
                }
            }
            // This is the first time we're seeing the file
            file_vec.push(FileWithAnnotatedLines {
                file: file,
                lines: vec![Line {
                                line_index: line_index,
                                annotations: vec![ann],
                            }],
                multiline_depth: 0,
            });
        }

        let mut output = vec![];
        let mut multiline_annotations = vec![];

        if let Some(ref cm) = cm {
            for span_label in spans {
                let mut loc = cm.look_up_span(span_label.span);

                // Watch out for "empty spans". If we get a span like 6..6, we
                // want to just display a `^` at 6, so convert that to
                // 6..7. This is degenerate input, but it's best to degrade
                // gracefully -- and the parser likes to supply a span like
                // that for EOF, in particular.
                if loc.begin == loc.end {
                    loc.end.column = loc.begin.column + 1;
                }

                let ann_type = if loc.begin.line != loc.end.line {
                    let ml = MultilineAnnotation {
                        depth: 1,
                        line_start: loc.begin.line,
                        line_end: loc.end.line,
                        start_col: loc.begin.column,
                        end_col: loc.end.column,
                        is_primary: span_label.style == SpanStyle::Primary,
                        label: span_label.label.clone(),
                    };
                    multiline_annotations.push((loc.file.clone(), ml.clone()));
                    AnnotationType::Multiline(ml)
                } else {
                    AnnotationType::Singleline
                };
                let ann = Annotation {
                    start_col: loc.begin.column,
                    end_col: loc.end.column,
                    is_primary: span_label.style == SpanStyle::Primary,
                    label: span_label.label.clone(),
                    annotation_type: ann_type,
                };

                if !ann.is_multiline() {
                    add_annotation_to_file(&mut output,
                                           loc.file,
                                           loc.begin.line,
                                           ann);
                }
            }
        }

        // Find overlapping multiline annotations, put them at different depths
        multiline_annotations.sort_by(|a, b| {
            (a.1.line_start, a.1.line_end).cmp(&(b.1.line_start, b.1.line_end))
        });
        for item in multiline_annotations.clone() {
            let ann = item.1;
            for item in multiline_annotations.iter_mut() {
                let ref mut a = item.1;
                // Move all other multiline annotations overlapping with this one
                // one level to the right.
                if &ann != a &&
                    num_overlap(ann.line_start, ann.line_end, a.line_start, a.line_end, true)
                {
                    a.increase_depth();
                } else {
                    break;
                }
            }
        }

        let mut max_depth = 0;  // max overlapping multiline spans
        for (file, ann) in multiline_annotations {
            if ann.depth > max_depth {
                max_depth = ann.depth;
            }
            add_annotation_to_file(&mut output, file.clone(), ann.line_start, ann.as_start());
            let middle = min(ann.line_start + 4, ann.line_end);
            for line in ann.line_start + 1..middle {
                add_annotation_to_file(&mut output, file.clone(), line, ann.as_line());
            }
            if middle < ann.line_end - 1 {
                for line in ann.line_end - 1..ann.line_end {
                    add_annotation_to_file(&mut output, file.clone(), line, ann.as_line());
                }
            }
            add_annotation_to_file(&mut output, file, ann.line_end, ann.as_end());
        }
        for file_vec in output.iter_mut() {
            file_vec.multiline_depth = max_depth;
        }
        output
    }

     fn render_source_line(&self,
                          buffer: &mut StyledBuffer,
                          file: &File,
                          line: &Line,
                          width_offset: usize,
                          code_offset: usize) -> Vec<(usize, Style)> {

        let source_string = file.source_line(line.line_index);

        let line_offset = buffer.num_lines();

        // First create the source line we will highlight.
        buffer.puts(line_offset, code_offset, &source_string, Style::Quotation);
        buffer.puts(line_offset,
                    0,
                    &((line.line_index + 1).to_string()),
                    Style::LineNumber);

        draw_col_separator(buffer, line_offset, width_offset - 2);

        // Special case when there's only one annotation involved, it is the start of a multiline
        // span and there's no text at the beginning of the code line. Instead of doing the whole
        // graph:
        //
        // 2 |   fn foo() {
        //   |  _^
        // 3 | |
        // 4 | | }
        //   | |_^ test
        //
        // we simplify the output to:
        //
        // 2 | / fn foo() {
        // 3 | |
        // 4 | | }
        //   | |_^ test
        if line.annotations.len() == 1 {
            if let Some(ref ann) = line.annotations.get(0) {
                if let AnnotationType::MultilineStart(depth) = ann.annotation_type {
                    if source_string[0..ann.start_col].trim() == "" {
                        let style = if ann.is_primary {
                            Style::UnderlinePrimary
                        } else {
                            Style::UnderlineSecondary
                        };
                        buffer.putc(line_offset,
                                    width_offset + depth - 1,
                                    '/',
                                    style);
                        return vec![(depth, style)];
                    }
                }
            }
        }

        // We want to display like this:
        //
        //      vec.push(vec.pop().unwrap());
        //      ---      ^^^               - previous borrow ends here
        //      |        |
        //      |        error occurs here
        //      previous borrow of `vec` occurs here
        //
        // But there are some weird edge cases to be aware of:
        //
        //      vec.push(vec.pop().unwrap());
        //      --------                    - previous borrow ends here
        //      ||
        //      |this makes no sense
        //      previous borrow of `vec` occurs here
        //
        // For this reason, we group the lines into "highlight lines"
        // and "annotations lines", where the highlight lines have the `^`.

        // Sort the annotations by (start, end col)
        let mut annotations = line.annotations.clone();
        annotations.sort();
        annotations.reverse();

        // First, figure out where each label will be positioned.
        //
        // In the case where you have the following annotations:
        //
        //      vec.push(vec.pop().unwrap());
        //      --------                    - previous borrow ends here [C]
        //      ||
        //      |this makes no sense [B]
        //      previous borrow of `vec` occurs here [A]
        //
        // `annotations_position` will hold [(2, A), (1, B), (0, C)].
        //
        // We try, when possible, to stick the rightmost annotation at the end
        // of the highlight line:
        //
        //      vec.push(vec.pop().unwrap());
        //      ---      ---               - previous borrow ends here
        //
        // But sometimes that's not possible because one of the other
        // annotations overlaps it. For example, from the test
        // `span_overlap_label`, we have the following annotations
        // (written on distinct lines for clarity):
        //
        //      fn foo(x: u32) {
        //      --------------
        //             -
        //
        // In this case, we can't stick the rightmost-most label on
        // the highlight line, or we would get:
        //
        //      fn foo(x: u32) {
        //      -------- x_span
        //      |
        //      fn_span
        //
        // which is totally weird. Instead we want:
        //
        //      fn foo(x: u32) {
        //      --------------
        //      |      |
        //      |      x_span
        //      fn_span
        //
        // which is...less weird, at least. In fact, in general, if
        // the rightmost span overlaps with any other span, we should
        // use the "hang below" version, so we can at least make it
        // clear where the span *starts*. There's an exception for this
        // logic, when the labels do not have a message:
        //
        //      fn foo(x: u32) {
        //      --------------
        //             |
        //             x_span
        //
        // instead of:
        //
        //      fn foo(x: u32) {
        //      --------------
        //      |      |
        //      |      x_span
        //      <EMPTY LINE>
        //
        let mut annotations_position = vec![];
        let mut line_len = 0;
        let mut p = 0;
        for (i, annotation) in annotations.iter().enumerate() {
            for (j, next) in annotations.iter().enumerate() {
                if overlaps(next, annotation, 0)  // This label overlaps with another one and both
                    && annotation.has_label()     // take space (they have text and are not
                    && j > i                      // multiline lines).
                    && p == 0  // We're currently on the first line, move the label one line down
                {
                    // This annotation needs a new line in the output.
                    p += 1;
                    break;
                }
            }
            annotations_position.push((p, annotation));
            for (j, next) in annotations.iter().enumerate() {
                if j > i  {
                    let l = if let Some(ref label) = next.label {
                        label.len() + 2
                    } else {
                        0
                    };
                    if (overlaps(next, annotation, l) // Do not allow two labels to be in the same
                                                     // line if they overlap including padding, to
                                                     // avoid situations like:
                                                     //
                                                     //      fn foo(x: u32) {
                                                     //      -------^------
                                                     //      |      |
                                                     //      fn_spanx_span
                                                     //
                        && annotation.has_label()    // Both labels must have some text, otherwise
                        && next.has_label())         // they are not overlapping.
                                                     // Do not add a new line if this annotation
                                                     // or the next are vertical line placeholders.
                        || (annotation.takes_space() // If either this or the next annotation is
                            && next.has_label())     // multiline start/end, move it to a new line
                        || (annotation.has_label()   // so as not to overlap the orizontal lines.
                            && next.takes_space())
                        || (annotation.takes_space()
                            && next.takes_space())
                    {
                        // This annotation needs a new line in the output.
                        p += 1;
                        break;
                    }
                }
            }
            if line_len < p {
                line_len = p;
            }
        }

        if line_len != 0 {
            line_len += 1;
        }

        // If there are no annotations or the only annotations on this line are
        // MultilineLine, then there's only code being shown, stop processing.
        if line.annotations.is_empty() || line.annotations.iter()
            .filter(|a| !a.is_line()).collect::<Vec<_>>().len() == 0
        {
            return vec![];
        }

        // Write the colunmn separator.
        //
        // After this we will have:
        //
        // 2 |   fn foo() {
        //   |
        //   |
        //   |
        // 3 |
        // 4 |   }
        //   |
        for pos in 0..line_len + 1 {
            draw_col_separator(buffer, line_offset + pos + 1, width_offset - 2);
            buffer.putc(line_offset + pos + 1,
                        width_offset - 2,
                        '|',
                        Style::LineNumber);
        }

        // Write the horizontal lines for multiline annotations
        // (only the first and last lines need this).
        //
        // After this we will have:
        //
        // 2 |   fn foo() {
        //   |  __________
        //   |
        //   |
        // 3 |
        // 4 |   }
        //   |  _
        for &(pos, annotation) in &annotations_position {
            let style = if annotation.is_primary {
                Style::UnderlinePrimary
            } else {
                Style::UnderlineSecondary
            };
            let pos = pos + 1;
            match annotation.annotation_type {
                AnnotationType::MultilineStart(depth) |
                AnnotationType::MultilineEnd(depth) => {
                    draw_range(buffer,
                               '_',
                               line_offset + pos,
                               width_offset + depth,
                               code_offset + annotation.start_col,
                               style);
                }
                _ => (),
            }
        }

        // Write the vertical lines for labels that are on a different line as the underline.
        //
        // After this we will have:
        //
        // 2 |   fn foo() {
        //   |  __________
        //   | |    |
        //   | |
        // 3 |
        // 4 | | }
        //   | |_
        for &(pos, annotation) in &annotations_position {
            let style = if annotation.is_primary {
                Style::UnderlinePrimary
            } else {
                Style::UnderlineSecondary
            };
            let pos = pos + 1;

            if pos > 1 && (annotation.has_label() || annotation.takes_space()) {
                for p in line_offset + 1..line_offset + pos + 1 {
                    buffer.putc(p,
                                code_offset + annotation.start_col,
                                '|',
                                style);
                }
            }
            match annotation.annotation_type {
                AnnotationType::MultilineStart(depth) => {
                    for p in line_offset + pos + 1..line_offset + line_len + 2 {
                        buffer.putc(p,
                                    width_offset + depth - 1,
                                    '|',
                                    style);
                    }
                }
                AnnotationType::MultilineEnd(depth) => {
                    for p in line_offset..line_offset + pos + 1 {
                        buffer.putc(p,
                                    width_offset + depth - 1,
                                    '|',
                                    style);
                    }
                }
                _ => (),
            }
        }

        // Write the labels on the annotations that actually have a label.
        //
        // After this we will have:
        //
        // 2 |   fn foo() {
        //   |  __________
        //   |      |
        //   |      something about `foo`
        // 3 |
        // 4 |   }
        //   |  _  test
        for &(pos, annotation) in &annotations_position {
            let style = if annotation.is_primary {
                Style::LabelPrimary
            } else {
                Style::LabelSecondary
            };
            let (pos, col) = if pos == 0 {
                (pos + 1, annotation.end_col + 1)
            } else {
                (pos + 2, annotation.start_col)
            };
            if let Some(ref label) = annotation.label {
                buffer.puts(line_offset + pos,
                            code_offset + col,
                            &label,
                            style);
            }
        }

        // Sort from biggest span to smallest span so that smaller spans are
        // represented in the output:
        //
        // x | fn foo()
        //   | ^^^---^^
        //   | |  |
        //   | |  something about `foo`
        //   | something about `fn foo()`
        annotations_position.sort_by(|a, b| {
            // Decreasing order
            a.1.len().cmp(&b.1.len()).reverse()
        });

        // Write the underlines.
        //
        // After this we will have:
        //
        // 2 |   fn foo() {
        //   |  ____-_____^
        //   |      |
        //   |      something about `foo`
        // 3 |
        // 4 |   }
        //   |  _^  test
        for &(_, annotation) in &annotations_position {
            let (underline, style) = if annotation.is_primary {
                ('^', Style::UnderlinePrimary)
            } else {
                ('-', Style::UnderlineSecondary)
            };
            for p in annotation.start_col..annotation.end_col {
                buffer.putc(line_offset + 1,
                            code_offset + p,
                            underline,
                            style);
            }
        }
        annotations_position.iter().filter_map(|&(_, annotation)| {
            match annotation.annotation_type {
                AnnotationType::MultilineStart(p) | AnnotationType::MultilineEnd(p) => {
                    let style = if annotation.is_primary {
                        Style::LabelPrimary
                    } else {
                        Style::LabelSecondary
                    };
                    Some((p, style))
                },
                _ => None
            }

        }).collect::<Vec<_>>()
    }

    fn get_max_line_num(&mut self, diagnostics: &[Diagnostic]) -> usize {
        if let Some(ref cm) = self.cm {
            diagnostics.iter().map(|d| {
                d.spans.iter().map(|span_label| {
                    cm.look_up_pos(span_label.span.high()).position.line
                }).max().unwrap_or(0)
            }).max().unwrap_or(0)
        } else { 0 }
    }

    /// Add a left margin to every line but the first, given a padding length and the label being
    /// displayed, keeping the provided highlighting.
    fn msg_to_buffer(&self,
                     buffer: &mut StyledBuffer,
                     msg: &Vec<(String, Style)>,
                     padding: usize,
                     label: &str,
                     override_style: Option<Style>) {

        // The extra 5 ` ` is padding that's always needed to align to the `note: `:
        //
        //   error: message
        //     --> file.rs:13:20
        //      |
        //   13 |     <CODE>
        //      |      ^^^^
        //      |
        //      = note: multiline
        //              message
        //   ++^^^----xx
        //    |  |   | |
        //    |  |   | magic `2`
        //    |  |   length of label
        //    |  magic `3`
        //    `max_line_num_len`
        let padding = (0..padding + label.len() + 5)
            .map(|_| " ")
            .collect::<String>();

        /// Return wether `style`, or the override if present and the style is `NoStyle`.
        fn style_or_override(style: Style, override_style: Option<Style>) -> Style {
            if let Some(o) = override_style {
                if style == Style::NoStyle {
                    return o;
                }
            }
            style
        }

        let mut line_number = 0;

        // Provided the following diagnostic message:
        //
        //     let msg = vec![
        //       ("
        //       ("highlighted multiline\nstring to\nsee how it ", Style::NoStyle),
        //       ("looks", Style::Highlight),
        //       ("with\nvery ", Style::NoStyle),
        //       ("weird", Style::Highlight),
        //       (" formats\n", Style::NoStyle),
        //       ("see?", Style::Highlight),
        //     ];
        //
        // the expected output on a note is (* surround the  highlighted text)
        //
        //        = note: highlighted multiline
        //                string to
        //                see how it *looks* with
        //                very *weird* formats
        //                see?
        for &(ref text, ref style) in msg.iter() {
            let lines = text.split('\n').collect::<Vec<_>>();
            if lines.len() > 1 {
                for (i, line) in lines.iter().enumerate() {
                    if i != 0 {
                        line_number += 1;
                        buffer.append(line_number, &padding, Style::NoStyle);
                    }
                    buffer.append(line_number, line, style_or_override(*style, override_style));
                }
            } else {
                buffer.append(line_number, text, style_or_override(*style, override_style));
            }
        }
    }

    fn emit_message_default(&mut self,
                            spans: &[SpanLabel],
                            msg: &Vec<(String, Style)>,
                            code: &Option<String>,
                            level: &Level,
                            max_line_num_len: usize,
                            is_secondary: bool)
                            -> io::Result<()> {
        let mut buffer = StyledBuffer::new();

        if is_secondary && spans.len() == 0 {
            // This is a secondary message with no span info
            for _ in 0..max_line_num_len {
                buffer.prepend(0, " ", Style::NoStyle);
            }
            draw_note_separator(&mut buffer, 0, max_line_num_len + 1);
            buffer.append(0, &level.to_string(), Style::HeaderMsg);
            buffer.append(0, ": ", Style::NoStyle);
            self.msg_to_buffer(&mut buffer, msg, max_line_num_len, "note", None);
        } else {
            buffer.append(0, &level.to_string(), Style::Level(level.clone()));
            if let Some(code) = code.as_ref() {
                buffer.append(0, "[", Style::Level(level.clone()));
                buffer.append(0, &code, Style::Level(level.clone()));
                buffer.append(0, "]", Style::Level(level.clone()));
            }
            buffer.append(0, ": ", Style::HeaderMsg);
            for &(ref text, _) in msg.iter() {
                buffer.append(0, text, Style::HeaderMsg);
            }
        }

        // Preprocess all the annotations so that they are grouped by file and by line number
        // This helps us quickly iterate over the whole message (including secondary file spans)
        let mut annotated_files = EmitterWriter::preprocess_annotations(self.cm, spans);

        // Make sure our primary file comes first
        let primary_lo = if let (Some(ref cm), Some(ref primary_span)) =
            (self.cm.as_ref(), spans.iter().find(|x| x.style == SpanStyle::Primary)) {
            cm.look_up_pos(primary_span.span.low())
        } else {
            // If we don't have span information, emit and exit
            emit_to_destination(&buffer.render(), level, &mut self.dst)?;
            return Ok(());
        };
        if let Ok(pos) =
            annotated_files.binary_search_by(|x| x.file.name().cmp(&primary_lo.file.name())) {
            annotated_files.swap(0, pos);
        }

        // Print out the annotate source lines that correspond with the error
        for annotated_file in annotated_files {
            // print out the span location and spacer before we print the annotated source
            // to do this, we need to know if this span will be primary
            let is_primary = primary_lo.file.name() == annotated_file.file.name();
            if is_primary {
                // remember where we are in the output buffer for easy reference
                let buffer_msg_line_offset = buffer.num_lines();

                buffer.prepend(buffer_msg_line_offset, "--> ", Style::LineNumber);
                let loc = primary_lo.clone();
                buffer.append(buffer_msg_line_offset,
                              &format!("{}:{}:{}", loc.file.name(), loc.position.line + 1, loc.position.column + 1),
                              Style::LineAndColumn);
                for _ in 0..max_line_num_len {
                    buffer.prepend(buffer_msg_line_offset, " ", Style::NoStyle);
                }
            } else {
                // remember where we are in the output buffer for easy reference
                let buffer_msg_line_offset = buffer.num_lines();

                // Add spacing line
                draw_col_separator(&mut buffer, buffer_msg_line_offset, max_line_num_len + 1);

                // Then, the secondary file indicator
                buffer.prepend(buffer_msg_line_offset + 1, "::: ", Style::LineNumber);
                buffer.append(buffer_msg_line_offset + 1,
                              annotated_file.file.name(),
                              Style::LineAndColumn);
                for _ in 0..max_line_num_len {
                    buffer.prepend(buffer_msg_line_offset + 1, " ", Style::NoStyle);
                }
            }

            // Put in the spacer between the location and annotated source
            let buffer_msg_line_offset = buffer.num_lines();
            draw_col_separator_no_space(&mut buffer, buffer_msg_line_offset, max_line_num_len + 1);

            // Contains the vertical lines' positions for active multiline annotations
            let mut multilines = HashMap::new();

            // Next, output the annotate source for this file
            for line_idx in 0..annotated_file.lines.len() {
                let previous_buffer_line = buffer.num_lines();

                let width_offset = 3 + max_line_num_len;
                let code_offset = if annotated_file.multiline_depth == 0 {
                    width_offset
                } else {
                    width_offset + annotated_file.multiline_depth + 1
                };

                let depths = self.render_source_line(&mut buffer,
                                                     &annotated_file.file,
                                                     &annotated_file.lines[line_idx],
                                                     width_offset,
                                                     code_offset);

                let mut to_add = HashMap::new();

                for (depth, style) in depths {
                    if multilines.get(&depth).is_some() {
                        multilines.remove(&depth);
                    } else {
                        to_add.insert(depth, style);
                    }
                }

                // Set the multiline annotation vertical lines to the left of
                // the code in this line.
                for (depth, style) in &multilines {
                    for line in previous_buffer_line..buffer.num_lines() {
                        draw_multiline_line(&mut buffer,
                                            line,
                                            width_offset,
                                            *depth,
                                            *style);
                    }
                }
                // check to see if we need to print out or elide lines that come between
                // this annotated line and the next one.
                if line_idx < (annotated_file.lines.len() - 1) {
                    let line_idx_delta = annotated_file.lines[line_idx + 1].line_index -
                                         annotated_file.lines[line_idx].line_index;
                    if line_idx_delta > 2 {
                        let last_buffer_line_num = buffer.num_lines();
                        buffer.puts(last_buffer_line_num, 0, "...", Style::LineNumber);

                        // Set the multiline annotation vertical lines on `...` bridging line.
                        for (depth, style) in &multilines {
                            draw_multiline_line(&mut buffer,
                                                last_buffer_line_num,
                                                width_offset,
                                                *depth,
                                                *style);
                        }
                    } else if line_idx_delta == 2 {
                        let unannotated_line = annotated_file.file
                            .source_line(annotated_file.lines[line_idx].line_index);

                        let last_buffer_line_num = buffer.num_lines();

                        buffer.puts(last_buffer_line_num,
                                    0,
                                    &(annotated_file.lines[line_idx + 1].line_index - 1)
                                        .to_string(),
                                    Style::LineNumber);
                        draw_col_separator(&mut buffer, last_buffer_line_num, 1 + max_line_num_len);
                        buffer.puts(last_buffer_line_num,
                                    code_offset,
                                    &unannotated_line,
                                    Style::Quotation);

                        for (depth, style) in &multilines {
                            draw_multiline_line(&mut buffer,
                                                last_buffer_line_num,
                                                width_offset,
                                                *depth,
                                                *style);
                        }
                    }
                }

                multilines.extend(&to_add);
            }
        }

        // final step: take our styled buffer, render it, then output it
        emit_to_destination(&buffer.render(), level, &mut self.dst)?;

        Ok(())
    }

    fn emit_messages_default(&mut self, msgs: &[Diagnostic]) {
        let max_line_num = self.get_max_line_num(msgs) + 1;
        let max_line_num_len = max_line_num.to_string().len();

        for msg in msgs {
            match self.emit_message_default(&msg.spans[..], &vec![(msg.message.clone(), Style::NoStyle)], &msg.code, &msg.level, max_line_num_len, false) {
                Ok(()) => (),
                Err(e) => panic!("failed to emit error: {}", e)
            }
        }

        match write!(&mut self.dst, "\n") {
            Err(e) => panic!("failed to emit error: {}", e),
            _ => {
                match self.dst.flush() {
                    Err(e) => panic!("failed to emit error: {}", e),
                    _ => (),
                }
            }
        }
    }
}


fn draw_col_separator(buffer: &mut StyledBuffer, line: usize, col: usize) {
    buffer.puts(line, col, "| ", Style::LineNumber);
}

fn draw_col_separator_no_space(buffer: &mut StyledBuffer, line: usize, col: usize) {
    draw_col_separator_no_space_with_style(buffer, line, col, Style::LineNumber);
}

fn draw_col_separator_no_space_with_style(buffer: &mut StyledBuffer,
                                          line: usize,
                                          col: usize,
                                          style: Style) {
    buffer.putc(line, col, '|', style);
}

fn draw_range(buffer: &mut StyledBuffer, symbol: char, line: usize,
              col_from: usize, col_to: usize, style: Style) {
    for col in col_from..col_to {
        buffer.putc(line, col, symbol, style);
    }
}

fn draw_note_separator(buffer: &mut StyledBuffer, line: usize, col: usize) {
    buffer.puts(line, col, "= ", Style::LineNumber);
}

fn draw_multiline_line(buffer: &mut StyledBuffer,
                       line: usize,
                       offset: usize,
                       depth: usize,
                       style: Style)
{
    buffer.putc(line, offset + depth - 1, '|', style);
}

fn num_overlap(a_start: usize, a_end: usize, b_start: usize, b_end:usize, inclusive: bool) -> bool {
    let extra = if inclusive {
        1
    } else {
        0
    };

    (a_start >= b_start && a_start < b_end + extra)
    || (b_start >= a_start && b_start < a_end + extra)
}
fn overlaps(a1: &Annotation, a2: &Annotation, padding: usize) -> bool {
    num_overlap(a1.start_col, a1.end_col + padding, a2.start_col, a2.end_col, false)
}

fn emit_to_destination(rendered_buffer: &Vec<Vec<StyledString>>,
                       lvl: &Level,
                       dst: &mut Destination)
                       -> io::Result<()> {
    use lock;

    // In order to prevent error message interleaving, where multiple error lines get intermixed
    // when multiple compiler processes error simultaneously, we emit errors with additional
    // steps.
    //
    // On Unix systems, we write into a buffered terminal rather than directly to a terminal. When
    // the .flush() is called we take the buffer created from the buffered writes and write it at
    // one shot.  Because the Unix systems use ANSI for the colors, which is a text-based styling
    // scheme, this buffered approach works and maintains the styling.
    //
    // On Windows, styling happens through calls to a terminal API. This prevents us from using the
    // same buffering approach.  Instead, we use a global Windows mutex, which we acquire long
    // enough to output the full error message, then we release.
    let _buffer_lock = lock::acquire_global_lock("rustc_errors");
    for line in rendered_buffer {
        for part in line {
            dst.apply_style(lvl.clone(), part.style)?;
            write!(dst, "{}", part.text)?;
            dst.reset_attrs()?;
        }
        write!(dst, "\n")?;
    }
    dst.flush()?;
    Ok(())
}

pub type BufferedStderr = term::Terminal<Output = BufferedWriter> + Send;

#[allow(dead_code)]
pub enum Destination {
    Terminal(Box<term::StderrTerminal>),
    BufferedTerminal(Box<BufferedStderr>),
    Raw(Box<Write + Send>),
}

use self::Destination::*;

/// Buffered writer gives us a way on Unix to buffer up an entire error message before we output
/// it.  This helps to prevent interleaving of multiple error messages when multiple compiler
/// processes error simultaneously
pub struct BufferedWriter {
    buffer: Vec<u8>,
}

impl BufferedWriter {
    // note: we use _new because the conditional compilation at its use site may make this
    // this function unused on some platforms
    fn _new() -> BufferedWriter {
        BufferedWriter { buffer: vec![] }
    }
}

impl Write for BufferedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for b in buf {
            self.buffer.push(*b);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut stderr = io::stderr();
        let result = (|| {
            stderr.write_all(&self.buffer)?;
            stderr.flush()
        })();
        self.buffer.clear();
        result
    }
}

impl Destination {
    #[cfg(not(windows))]
    /// When not on Windows, prefer the buffered terminal so that we can buffer an entire error
    /// to be emitted at one time.
    fn from_stderr() -> Destination {
        let stderr: Option<Box<BufferedStderr>> =
            term::TerminfoTerminal::new(BufferedWriter::_new())
                .map(|t| Box::new(t) as Box<BufferedStderr>);

        match stderr {
            Some(t) => BufferedTerminal(t),
            None => Raw(Box::new(io::stderr())),
        }
    }

    #[cfg(windows)]
    /// Return a normal, unbuffered terminal when on Windows.
    fn from_stderr() -> Destination {
        let stderr: Option<Box<term::StderrTerminal>> = term::TerminfoTerminal::new(io::stderr())
            .map(|t| Box::new(t) as Box<term::StderrTerminal>)
            .or_else(|| {
                term::WinConsole::new(io::stderr())
                    .ok()
                    .map(|t| Box::new(t) as Box<term::StderrTerminal>)
            });

        match stderr {
            Some(t) => Terminal(t),
            None => Raw(Box::new(io::stderr())),
        }
    }

    fn apply_style(&mut self, lvl: Level, style: Style) -> io::Result<()> {
        match style {
            Style::LineAndColumn => {}
            Style::LineNumber => {
                self.start_attr(term::Attr::Bold)?;
                if cfg!(windows) {
                    self.start_attr(term::Attr::ForegroundColor(term::color::BRIGHT_CYAN))?;
                } else {
                    self.start_attr(term::Attr::ForegroundColor(term::color::BRIGHT_BLUE))?;
                }
            }
            Style::Quotation => {}
            Style::HeaderMsg => {
                self.start_attr(term::Attr::Bold)?;
                if cfg!(windows) {
                    self.start_attr(term::Attr::ForegroundColor(term::color::BRIGHT_WHITE))?;
                }
            }
            Style::UnderlinePrimary | Style::LabelPrimary => {
                self.start_attr(term::Attr::Bold)?;
                self.start_attr(term::Attr::ForegroundColor(lvl.color()))?;
            }
            Style::UnderlineSecondary |
            Style::LabelSecondary => {
                self.start_attr(term::Attr::Bold)?;
                if cfg!(windows) {
                    self.start_attr(term::Attr::ForegroundColor(term::color::BRIGHT_CYAN))?;
                } else {
                    self.start_attr(term::Attr::ForegroundColor(term::color::BRIGHT_BLUE))?;
                }
            }
            Style::NoStyle => {}
            Style::Level(l) => {
                self.start_attr(term::Attr::Bold)?;
                self.start_attr(term::Attr::ForegroundColor(l.color()))?;
            }
            Style::Highlight => self.start_attr(term::Attr::Bold)?,
        }
        Ok(())
    }

    fn start_attr(&mut self, attr: term::Attr) -> io::Result<()> {
        match *self {
            Terminal(ref mut t) => {
                t.attr(attr)?;
            }
            BufferedTerminal(ref mut t) => {
                t.attr(attr)?;
            }
            Raw(_) => {}
        }
        Ok(())
    }

    fn reset_attrs(&mut self) -> io::Result<()> {
        match *self {
            Terminal(ref mut t) => {
                t.reset()?;
            }
            BufferedTerminal(ref mut t) => {
                t.reset()?;
            }
            Raw(_) => {}
        }
        Ok(())
    }
}

impl Write for Destination {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match *self {
            Terminal(ref mut t) => t.write(bytes),
            BufferedTerminal(ref mut t) => t.write(bytes),
            Raw(ref mut w) => w.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Terminal(ref mut t) => t.flush(),
            BufferedTerminal(ref mut t) => t.flush(),
            Raw(ref mut w) => w.flush(),
        }
    }
}
