//! Local event buffer

use std::marker::PhantomData;

/// A local buffer of events
///
/// TODO implement
#[derive(Debug, Clone)]
pub struct Local<T> {
    _inner: PhantomData<T>,
}
