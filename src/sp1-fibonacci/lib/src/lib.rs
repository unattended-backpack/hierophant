//! SP1-side glue for the shared fibonacci test program. The only thing in
//! this crate is the Solidity ABI layout for the public values the SP1 guest
//! commits; `fibonacci` itself lives in the shared no_std `fibonacci` crate
//! at `src/fibonacci/`, and is consumed directly by the guest and host.
//! Having the shared logic in one place (and not re-exported from here) is
//! the whole point of that crate; please don't re-introduce a local copy.

use alloy_sol_types::sol;

sol! {
    /// The public values encoded as a struct that can be easily deserialized
    /// inside Solidity.
    struct PublicValuesStruct {
        uint32 n;
        uint32 a;
        uint32 b;
    }
}
