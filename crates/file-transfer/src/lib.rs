//! Transferência de arquivos e pastas (Fase 5).

/// Aceita apenas caminhos relativos sem `..`, drive, UNC ou segmentos vazios.
pub fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|c| c == ".." || c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_traversal_and_absolute() {
        for p in [
            "../x",
            "a/../b",
            "/etc/x",
            "C:\\x",
            "\\\\srv\\s",
            "a//b",
            "",
        ] {
            assert!(!is_safe_relative(p), "{p}");
        }
        assert!(is_safe_relative("pasta/ação.txt"));
    }
}
