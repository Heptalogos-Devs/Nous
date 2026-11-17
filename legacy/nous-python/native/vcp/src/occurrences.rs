//! Shared occurrence traversal preserves input order and bounded cancellation latency.
pub(crate) fn for_each_pair<T>(
    rows: &[T],
    cancelled: crate::CancellationCheck<'_>,
    mut visit: impl FnMut(&T, &T) -> Result<(), String>,
) -> Result<(), String> {
    let mut work = 0usize;
    crate::check_cancelled(cancelled)?;
    for (i, left) in rows.iter().enumerate() {
        for right in &rows[i + 1..] {
            if work % 1024 == 0 {
                crate::check_cancelled(cancelled)?;
            }
            visit(left, right)?;
            work += 1;
        }
    }
    Ok(())
}
