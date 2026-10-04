//! T-038 (decision #41 supersedes #39): doc comments are never read as code, and not read for
//! anything else. Each sibling module holds the source of a removed T-035/T-036 doc fixture
//! or a b8b0b81 probe, shapes rustdoc 1.99 reads as code blocks; with `[lib] doctest = false`
//! pinned and no `--doc` in the workflows none of them becomes an executable, so all pass.
//! Code-looking text in every doc form must not count either:
//! #[cfg(test)] mod t { #[test] fn a() {} }
//! #![cfg(test)]
/*! #[path = "../other/t.rs"] mod t;
    include!("../other/t.rs");
    #[cfg_attr(test, test)] fn b() {}
*/
#![doc = "#[test] #[cfg(test)] include!(\"x.rs\")"]
#![doc = include_str!("x.md")]

pub mod ok_doc_list_continuation;
pub mod ok_text_fence;
pub mod probe_a_tab;
pub mod probe_b_rawadd;
pub mod probe_c2_outer_inner_space;
pub mod probe_c_outer_inner;
pub mod probe_d_footnote;
pub mod probe_e_deflist;
pub mod probe_f_html;
pub mod probe_g_xesc;
pub mod probe_h_cr;
pub mod probe_i_block_inconsistent;
pub mod probe_j2_block_regular;
pub mod probe_j_block_blank_mid;
pub mod probe_k_marker4;
pub mod probe_l_htmlcomment;
pub mod v_block_doc_fence;
pub mod v_block_inner_doc_fence;
pub mod v_doc_attr_fence;
pub mod v_doc_empty_item_code;
pub mod v_doc_empty_ordered_item_code;
pub mod v_doc_include_str;
pub mod v_doc_indented_block;
pub mod v_doc_indented_no_space;
pub mod v_doc_lazy_continuation;
pub mod v_doc_list_continuation;
pub mod v_doc_list_marker_code;
pub mod v_doc_list_marker_tab;
pub mod v_doc_quote_code;
pub mod v_doc_quote_empty_code;
pub mod v_doc_quote_lazy;
pub mod v_doc_setext_code;
pub mod v_doc_star_break_code;
pub mod v_doc_thematic_break_code;
pub mod v_fence_bare;
pub mod v_fence_rust;
pub mod v_fence_tilde;
pub mod v_inner_doc_fence;
pub mod v_inner_doc_indented_block;

/// #[test]
/// # [test]
/// #[tokio::test]
/// #[test_case(1)]
/// #[cfg(all(windows, test))]
/// #[cfg_attr(test, path = "../other/t.rs")]
/// include!("../other/t.rs"); r#include!("x.rs"); use core::include as pull;
/// "an unclosed string, ' a stray quote, /* an unclosed comment opener
pub fn outer_line_docs() {}

/** include!("../other/t.rs");
 * #[cfg(test)] mod t {}
 * #[path = "t.rs"] mod u;
 */
pub fn outer_block_doc() {}

#[doc = "#[cfg(test)]"]
#[doc = "#[test] fn a() {}"]
#[doc = "include!(\"../other/t.rs\");"]
pub fn outer_doc_attrs() {}

#[doc = include_str!("x.md")]
pub fn outer_doc_include_str() {}

#[doc = concat!("#[cfg(", "test)]")]
pub fn outer_doc_concat() {}
