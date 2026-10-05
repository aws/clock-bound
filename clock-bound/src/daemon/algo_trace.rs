//! Tracing macro to emit the synchronization internals into `algo_analysis.log`.

/// A tracepoint into `algo_analysis.log`: `algo_trace!(Kind { field: value, … })`.
///
/// `Kind` is a record type of `crate::daemon::algo_analysis` (e.g. "theta").
///
/// With the `algorithm` feature, the record is built and emitted through the structured
/// logging. Without it the macro expands to a use of each value and nothing else. The benefit is
/// that the algorithm carries no `cfg` and no lint exception for values that only the analysis
/// reads.
#[cfg(feature = "algorithm")]
macro_rules! algo_trace {
    ($kind:ident { $($field:ident : $value:expr),* $(,)? }) => {
        $crate::daemon::logging::emit_event(&$crate::daemon::algo_analysis::$kind { $($field: $value),* })
    };
}
#[cfg(not(feature = "algorithm"))]
macro_rules! algo_trace {
    ($kind:ident { $($field:ident : $value:expr),* $(,)? }) => {{
        $( let _ = &$value; )*
    }};
}
pub(crate) use algo_trace;
