extern crate codemap_diagnostic;
extern crate codemap;
use codemap::{ CodeMap };
use codemap_diagnostic::{ Level, SpanLabel, SpanStyle, Diagnostic, ColorConfig, Emitter };


fn main() {
    let file = r#"
pub fn look_up_pos(&self, pos: Pos) -> Loc {
    let file = self.find_file(pos);
    let position = file.find_line_col(pos);
    Loc { file, position }
}
"#;

    let mut codemap = CodeMap::new();
    let file_span = codemap.add_file("test.rs".to_owned(), file.to_owned()).span;
    let fn_span = file_span.subspan(8, 19);
    let ret_span = file_span.subspan(40, 43);
    let var_span = file_span.subspan(54, 58);

    let sl = SpanLabel { span: fn_span, style: SpanStyle::Primary, label:Some("function name".to_owned()) };
    let sl2 = SpanLabel { span: ret_span, style: SpanStyle::Primary, label:Some("returns".to_owned()) };
    let d1 = Diagnostic { level:Level::Error, message:"Test error".to_owned(), code:Some("C000".to_owned()), spans: vec![sl, sl2] };

    let sl3 = SpanLabel { span: var_span, style: SpanStyle::Primary, label:Some("variable".to_owned()) };
    let d2 = Diagnostic { level:Level::Warning, message:"Test warning".to_owned(), code:Some("W000".to_owned()), spans: vec![sl3] };

    let d3 = Diagnostic { level: Level::Help, message:"Help message".to_owned(), code: None, spans: vec![] };

    let mut emitter = Emitter::stderr(ColorConfig::Auto, Some(&codemap));
    emitter.emit(&[d1, d2, d3]);
}
