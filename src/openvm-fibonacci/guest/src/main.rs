// OpenVM fibonacci guest; parity with src/sp1-fibonacci/program and
// src/risc0-fibonacci/guest.
//
// Reads `n: u32` from the host's first StdIn stream, computes the nth
// Fibonacci number iteratively, and reveals (n, a, b) as user public values
// where:
//   - a = fib(n - 1)
//   - b = fib(n)
//
// The actual arithmetic lives in the shared `fibonacci` crate so it's
// identical to what the SP1 and RISC Zero guests run; the demonstration
// point of having `src/fibonacci` as a shared no_std crate.
//
// OpenVM public values are a fixed 32-byte address space (8 u32 slots by
// default); reveal_u32(x, i) places x at byte offset i*4 little-endian, so
// the proof's user public values decode as [n, a, b, 0, 0, 0, 0, 0].

#![no_main]
#![no_std]

openvm::entry!(main);

use fibonacci::fibonacci;
use openvm::io::{read, reveal_u32};

fn main() {
    let n: u32 = read();

    let (a, b) = fibonacci(n);

    reveal_u32(n, 0);
    reveal_u32(a, 1);
    reveal_u32(b, 2);
}
