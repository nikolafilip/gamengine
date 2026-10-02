//! gm-model: avatar models (`docs/MODELS.md`).
//!
//! - [`rig`]: the standard 24-bone rig; a bone is its pivot.
//! - [`format`]: the ingested `.gmm` container, its strict reader and its limits.
//! - [`bc1`]: BC1 decoding for GPUs without it, previews and tests.
//! - [`pose`]: poses and skinning matrices.
//! - [`anim`]: the shared, procedural animation set.
//! - [`mannequin`]: the generated stand-in figure and its finer variants.
//!
//! Small on purpose: the client links this crate, and nothing here parses an upload. Uploads
//! are `gm-ingest`'s business, in a worker process of the hub.
#![forbid(unsafe_code)]

pub mod anim;
pub mod bc1;
pub mod format;
pub mod mannequin;
pub mod pose;
pub mod rig;

pub use format::{Model, ModelError, ModelId, Vertex, id_from_hex, id_hex, limits, model_id};
pub use pose::{Pose, skin_matrices};
pub use rig::BONES;
