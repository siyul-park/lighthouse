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
