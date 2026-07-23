pub const fn get_core_version() -> &'static str {
    "0.0.0"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_source_placeholder_version() {
        assert_eq!(get_core_version(), "0.0.0");
    }
}
