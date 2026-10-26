//! Private Opus packet DSP, adapted under MIT/BSD; see ../NOTICE.md.
#![allow(
    dead_code,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    rustdoc::broken_intra_doc_links
)]
mod celt;
#[cfg(test)]
mod conformance;
mod decoder;
mod packet;
mod range;
mod silk;
mod stream;
use super::error::CodecError as CadenceError;
pub(super) use stream::{Core, Head};
type Result<T, E = CadenceError> = std::result::Result<T, E>;
mod debug {
    pub(super) fn init() {}
    pub(super) struct Flags {
        pub celt_band_trace: bool,
        pub silk_c_fs_debug: bool,
        pub silk_dbg: bool,
    }
    pub(super) fn flags() -> Flags {
        Flags {
            celt_band_trace: false,
            silk_c_fs_debug: false,
            silk_dbg: false,
        }
    }
}
