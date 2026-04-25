// Bonsai-shaped REST surface. Lets a client using the `bonsai-sdk` crate point
// at this hierophant instead of Risc Zero's hosted Bonsai service and drive the
// same request/upload/poll flow. The endpoints live under `/bonsai/*` on the
// same Axum app as the SP1 HTTP endpoints.
//
// Internally this module translates Bonsai's image-id + input-id semantics into
// the VM-agnostic `route_risc0_proof` call on `ProofRouter`; the contemplant
// never learns it was a Bonsai client that initiated the proof.

mod router;
mod state;
mod types;

pub use router::bonsai_routes;
pub use state::BonsaiState;
