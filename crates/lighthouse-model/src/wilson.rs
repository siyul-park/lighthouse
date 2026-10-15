//! The Wilson score interval of a proportion: the one the metrics of an
//! evaluation and the abstain band of a trained model both bound by.

/// z for a 95% interval.
const Z: f64 = 1.959_963_984_540_054;

/// The 95% interval of `hits` out of `of` (weights are fine: they need not be
/// whole numbers); `of` must be above zero.
pub fn wilson(hits: f64, of: f64) -> (f64, f64) {
    let p = hits / of;
    let z2 = Z * Z;
    let centre = (p + z2 / (2.0 * of)) / (1.0 + z2 / of);
    let half = Z * (p * (1.0 - p) / of + z2 / (4.0 * of * of)).sqrt() / (1.0 + z2 / of);
    ((centre - half).max(0.0), (centre + half).min(1.0))
}
