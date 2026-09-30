/// Wrap an index by `delta` steps within `[0, len)`, saturating to 0 when the collection is empty. The
/// single source for the single-choice modals' highlight movement — the command menu, the picker, the
/// wizard's kind list, and the approval and plan boxes — so they all wrap consistently instead of some
/// wrapping and some clamping.
pub fn wrapping_step(index: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (index as i32 + delta).rem_euclid(len as i32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_step_wraps_both_directions_and_handles_empty() {
        // Wrap past the bottom edge back to the top, and past the top back to the bottom.
        assert_eq!(wrapping_step(2, 1, 3), 0);
        assert_eq!(wrapping_step(0, -1, 3), 2);
        // In-range moves are plain steps.
        assert_eq!(wrapping_step(1, 1, 3), 2);
        // A delta larger than the length still lands in range.
        assert_eq!(wrapping_step(0, -3, 3), 0);
        assert_eq!(wrapping_step(1, 5, 3), 0);
        // An empty collection saturates to 0 instead of panicking on the modulus.
        assert_eq!(wrapping_step(0, 1, 0), 0);
        assert_eq!(wrapping_step(5, -1, 0), 0);
    }
}
