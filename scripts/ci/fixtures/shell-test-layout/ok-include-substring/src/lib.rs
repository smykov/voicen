//! Identifiers that merely contain "include", and the word in comments, strings and docs:
//! none of them is the `include` token, so none reaches include!.
// include!("../other/t.rs") in a plain comment is never compiled.
/// Call [`included`] to include nothing; `include!` named in doc text is text.
pub fn included() -> usize {
    let include_count = 1;
    let not_include = 2;
    let includes = "include!(\"../other/t.rs\")";
    let INCLUDE_ALL = r#"include"#;
    let _ = (includes, INCLUDE_ALL);
    include_count + not_include
}

pub static DATA: &str = include_str!("data.txt");
pub static BYTES: &[u8] = include_bytes!("data.txt");
