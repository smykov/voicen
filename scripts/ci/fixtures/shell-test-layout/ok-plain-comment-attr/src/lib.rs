// Plain comments that mention test attributes and fences are not code:
// #[test]
// #[cfg(test)]
// ```rust
// let x = 1;
// ```
pub fn f() {} // see #[test] in tests/ instead; #[cfg(test)] is not allowed here
