//! `skip_serializing_if` predicates for Go `omitempty` fields.

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

pub(crate) fn is_zero(n: &i64) -> bool {
    *n == 0
}

pub(crate) fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}
