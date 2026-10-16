//! What a trained model's out-of-fold probabilities say about it: the band
//! in which they can be trusted, and how well they rank.

use lighthouse_model::wilson;
use serde::Serialize;

use crate::rows::Line;

/// The decimals a probability is given in.
const DECIMALS: f64 = 1e4;

/// The band a card records, not applied by the host: from `upper` up a fail
/// is as precise as `precision` asks, from `lower` down a pass is. `None`
/// when no threshold reaches it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Band {
    pub upper: Option<f64>,
    pub lower: Option<f64>,
    pub precision: f64,
}

/// The band of probabilities `p` of `rows` failing: the smallest threshold
/// from which the weighted precision of a fail has a Wilson 95% lower bound
/// of at least `precision`, and likewise, from the other side, of a pass.
pub fn band(p: &[f64], rows: &[&Line], precision: f64) -> Band {
    let scored: Vec<(f64, &Line)> = p
        .iter()
        .map(|p| rounded(*p))
        .zip(rows.iter().copied())
        .collect();
    let mut thresholds: Vec<f64> = scored.iter().map(|(p, _)| *p).collect();
    thresholds.sort_by(f64::total_cmp);
    thresholds.dedup();
    let reached = |inside: fn(f64, f64) -> bool, hit: fn(&Line) -> f64, t: f64| {
        let (hits, of) = scored
            .iter()
            .filter(|(p, _)| inside(*p, t))
            .fold((0.0, 0.0), |(hits, of), (_, row)| {
                (hits + row.weight * hit(row), of + row.weight)
            });
        of > 0.0 && wilson(hits, of).0 >= precision
    };
    Band {
        upper: thresholds
            .iter()
            .copied()
            .find(|&t| reached(|p, t| p >= t, |row| row.label, t)),
        lower: thresholds
            .iter()
            .rev()
            .copied()
            .find(|&t| reached(|p, t| p <= t, |row| 1.0 - row.label, t)),
        precision,
    }
}

/// The probability that a fail is given a higher probability than a pass,
/// ties counting half; `None` when a class is missing.
pub fn auroc(p: &[f64], rows: &[&Line]) -> Option<f64> {
    let of = |label: f64| -> Vec<f64> {
        p.iter()
            .zip(rows)
            .filter(|(_, r)| r.label == label)
            .map(|(p, _)| *p)
            .collect()
    };
    let (fails, passes) = (of(1.0), of(0.0));
    if fails.is_empty() || passes.is_empty() {
        return None;
    }
    let wins: f64 = fails
        .iter()
        .flat_map(|f| passes.iter().map(move |p| (f, p)))
        .map(|(f, p)| match f.total_cmp(p) {
            std::cmp::Ordering::Greater => 1.0,
            std::cmp::Ordering::Equal => 0.5,
            std::cmp::Ordering::Less => 0.0,
        })
        .sum();
    Some(wins / (fails.len() * passes.len()) as f64)
}

/// `p` to four decimals.
pub fn rounded(p: f64) -> f64 {
    (p * DECIMALS).round() / DECIMALS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: f64) -> Line {
        Line {
            label,
            weight: 1.0,
            ..Line::default()
        }
    }

    #[test]
    fn the_band_starts_where_the_wilson_bound_of_the_precision_is_reached() {
        // 40 fails at 0.9, 40 passes at 0.1, and a mixed middle at 0.5. 40 of
        // 40 has a Wilson lower bound of 0.912, above 0.9; with the middle
        // (10 fails, 10 passes) in, 50 of 60 is 0.72 at best.
        let mut rows: Vec<Line> = Vec::new();
        let mut p = Vec::new();
        for (score, label, n) in [
            (0.9, 1.0, 40),
            (0.5, 1.0, 10),
            (0.5, 0.0, 10),
            (0.1, 0.0, 40),
        ] {
            for _ in 0..n {
                rows.push(row(label));
                p.push(score);
            }
        }
        let refs: Vec<&Line> = rows.iter().collect();
        let band = band(&p, &refs, 0.9);

        assert_eq!(band.upper, Some(0.9));
        assert_eq!(band.lower, Some(0.1));
    }

    #[test]
    fn a_band_is_none_when_no_threshold_reaches_the_precision() {
        let rows: Vec<Line> = (0..20).map(|i| row(f64::from(i % 2))).collect();
        let refs: Vec<&Line> = rows.iter().collect();
        let band = band(&[0.5; 20], &refs, 0.9);

        assert_eq!((band.upper, band.lower), (None, None));
    }

    #[test]
    fn auroc_counts_ties_half_and_needs_both_classes() {
        let rows = [row(1.0), row(0.0), row(1.0)];
        let refs: Vec<&Line> = rows.iter().collect();
        // Fails 0.8 and 0.4 against the pass 0.4: one win, one tie.
        assert_eq!(auroc(&[0.8, 0.4, 0.4], &refs), Some(0.75));
        assert_eq!(auroc(&[0.8, 0.4], &refs[..1]), None);
    }
}
