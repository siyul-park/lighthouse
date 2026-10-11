pub fn run(x: u8) -> u8 {
    x + 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn adds() {
        assert_eq!(super::run(1), fixtures::sample());
    }
}
