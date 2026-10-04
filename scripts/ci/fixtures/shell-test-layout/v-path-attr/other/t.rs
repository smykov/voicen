//! Compiled into the crate from outside src; the guard never reads this file.
#[cfg(test)]
mod x {
    #[test]
    fn a() {}
}
