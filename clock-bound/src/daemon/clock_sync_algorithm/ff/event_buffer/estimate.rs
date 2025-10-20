//! An estimate event buffer

use std::marker::PhantomData;

/// An estimate ring buffer
///
/// TODO: Implement
#[derive(Debug, Clone)]
pub struct Estimate<T> {
    _phantom: PhantomData<T>,
}
