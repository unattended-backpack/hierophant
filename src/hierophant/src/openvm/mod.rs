// OpenVM-shaped REST surface. There is no third-party wire protocol to be a
// drop-in replacement for here (OpenVM has no public bonsai-sdk analog), so
// this surface deliberately mirrors the Bonsai module's upload/create/poll/
// download flow, just with OpenVM nouns: programs (raw guest ELFs), inputs
// (StdIn hint streams), and proof jobs in one of three modes (app STARK,
// aggregated root STARK, or halo2-wrapped EVM proof). The endpoints live
// under `/openvm/*` on the same Axum app as the SP1 HTTP endpoints.
//
// Internally this module translates program-id + input-id semantics into the
// VM-agnostic `route_openvm_proof` call on `ProofRouter`; the contemplant
// never learns which client surface initiated the proof.

mod router;
mod state;
mod types;
pub mod verify;

pub use router::openvm_routes;
pub use state::OpenVmState;
