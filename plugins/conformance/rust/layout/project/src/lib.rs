/// Adds two numbers.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn private_sum(values: &[i32]) -> i32 {
    values.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(1, 2), 3);
    }

    #[test]
    fn adds_many() {
        for (a, b, sum) in [(1, 2, 3), (2, 3, 5)] {
            assert_eq!(add(a, b), sum);
        }
    }

    #[test]
    fn sums_privately() {
        assert_eq!(private_sum(&[1, 2]), 3);
    }
}

#[cfg(test)]
mod more;
