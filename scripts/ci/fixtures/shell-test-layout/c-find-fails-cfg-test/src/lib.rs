// T-038: a find error must fail closed (exit 3) and never skip the source scan. This shell
// holds a cfg(test) module, so a guard that skipped the scan on a find error would pass
// (exit 0) and one that scanned anyway would report it (exit 1); only exit 3 is right.
#[cfg(test)]
mod t {}

pub fn f() {}
