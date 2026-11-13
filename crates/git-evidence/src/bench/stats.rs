//! The median and the median absolute deviation the bench reports for each
//! figure. Each takes at least one value.

/// The middle value of `values`; the mean of the two middle values when
/// their count is even.
pub(crate) fn median(values: &[f64]) -> f64 {
    debug_assert!(!values.is_empty(), "the median of no values");
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    }
}

/// The median of `values` and their median absolute deviation from it: a
/// centre and a spread that one wild value cannot move.
pub(crate) fn median_and_mad(values: &[f64]) -> (f64, f64) {
    let centre = median(values);
    let deviations: Vec<f64> = values.iter().map(|v| (v - centre).abs()).collect();
    (centre, median(&deviations))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert!((median(&[4.0, 1.0, 3.0, 2.0]) - 2.5).abs() < 1e-12);
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn one_wild_value_moves_neither_the_median_nor_the_mad() {
        let (centre, mad) = median_and_mad(&[10.0, 11.0, 9.0, 10.0, 1000.0]);
        assert!((centre - 10.0).abs() < 1e-12);
        assert!((mad - 1.0).abs() < 1e-12);
    }
}
