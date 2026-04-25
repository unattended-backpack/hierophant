//! Shared no_std fibonacci implementation used by both the SP1 and RISC Zero
//! integration-test guests. Keep this the single source of truth; if you
//! find yourself writing another `fibonacci` elsewhere in the tree, fix the
//! caller to depend on this crate instead.

#![no_std]

/// Computes `(fib(n - 1), fib(n))` with u32 wrapping arithmetic.
///
/// Special cases: for `n == 0` returns `(0, 1)` since by convention
/// `fib(-1) = 1`; callers interested only in the n-th value use the
/// tuple's second element.
pub fn fibonacci(n: u32) -> (u32, u32) {
    let mut a: u32 = 0;
    let mut b: u32 = 1;
    for _ in 0..n {
        let next = a.wrapping_add(b);
        a = b;
        b = next;
    }
    (a, b)
}
