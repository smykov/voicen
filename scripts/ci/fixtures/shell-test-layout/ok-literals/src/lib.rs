//! Every literal or comment below holds text that is a finding when it is not lexed:
//! #[test], #[cfg(test)], a doc fence, or a `"` that would open a string.
pub const S1: &str = "#[test] #[cfg(test)]";
pub const S2: &str = "esc \" #[test] \"";
pub const S3: &str = r#"raw " #[test] "#;
pub const S4: &[u8] = br#"byte raw " #[cfg(test)] "#;
pub const S5: &str = "multi
#[test]
line";
pub const C1: char = '"'; pub const S6: &str = "#[test]";
pub const C2: u8 = b'"'; pub const S7: &str = "#[test]";
pub const C3: char = '\''; pub const C4: char = '\u{1F600}';
pub fn life<'a>(x: &'a str) -> &'a str { 'outer: loop { break 'outer; } x }
/* outer /* inner */ #[test] */
/*** ```rust ***/
/// It's "quoted, and `#[test]` in a doc is prose.
#[deprecated(note = "a ] in a string")]
pub fn f() {}
pub const S8: &core::ffi::CStr = cr#"c raw " #[test] "#;
/* a plain block comment with a fence: ``` and #[cfg(test)] */
