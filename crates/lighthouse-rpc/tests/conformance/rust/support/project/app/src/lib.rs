pub fn run(x: u8) -> u8 {
    x + shared::shared()
}

#[cfg(test)]
mod tests {
    #[test]
    fn adds() {
        assert_eq!(super::run(1), fixtures::sample());
    }
}
