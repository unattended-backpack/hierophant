// RISC Zero fibonacci guest; parity with src/sp1-fibonacci/program.
//
// Reads `n: u32` from the host's ExecutorEnv input, computes the nth Fibonacci
// number iteratively, and commits (n, a, b) to the journal where:
//   - a = fib(n - 1)
//   - b = fib(n)
//
// The actual arithmetic lives in the shared `fibonacci` crate so it's
// identical to what the SP1 guest runs; the demonstration point of having
// `src/fibonacci` as a shared no_std crate.

#![no_main]
risc0_zkvm::guest::entry!(main);

use fibonacci::fibonacci;
use risc0_zkvm::guest::env;

fn main() {
    let n: u32 = env::read();

    let (a, b) = fibonacci(n);

    env::commit(&n);
    env::commit(&a);
    env::commit(&b);
}
