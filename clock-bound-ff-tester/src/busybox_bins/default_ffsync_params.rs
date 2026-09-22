//! Print the complete production parameter file for `repro-replay --ffsync-params`.
//!
//! The parameter file schema is strict (every key required), so this is the file to start
//! from: copy, edit a value, replay. See [`crate::ffsync_params`].

use crate::ffsync_params;

pub fn main() {
    print!("{}", ffsync_params::default_file());
}
