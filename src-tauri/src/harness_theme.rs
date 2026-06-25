use crate::ir::Harness;

pub struct HarnessTheme {
    pub label: &'static str,
    pub color: &'static str,
    pub glyph: &'static str,
}

pub fn harness_theme(h: &Harness) -> HarnessTheme {
    match h {
        Harness::ClaudeCode => HarnessTheme {
            label: "Claude",
            color: "#3dffa0",
            glyph: "◆",
        },
        Harness::Codex => HarnessTheme {
            label: "Codex",
            color: "#b98cff",
            glyph: "▣",
        },
        _ => HarnessTheme {
            label: "Other",
            color: "#8a9099",
            glyph: "●",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Harness;

    #[test]
    fn claude_code_theme() {
        let t = harness_theme(&Harness::ClaudeCode);
        assert_eq!(t.label, "Claude");
        assert_eq!(t.color, "#3dffa0");
        assert_eq!(t.glyph, "◆");
    }

    #[test]
    fn codex_theme() {
        let t = harness_theme(&Harness::Codex);
        assert_eq!(t.label, "Codex");
        assert_eq!(t.color, "#b98cff");
        assert_eq!(t.glyph, "▣");
    }

    #[test]
    fn generic_theme_is_neutral() {
        let t = harness_theme(&Harness::Generic("mycustom".to_string()));
        assert_eq!(t.color, "#8a9099");
        assert_eq!(t.glyph, "●");
        // label is non-empty
        assert!(!t.label.is_empty());
    }

    #[test]
    fn cursor_theme_is_neutral() {
        let t = harness_theme(&Harness::Cursor);
        assert_eq!(t.color, "#8a9099");
        assert_eq!(t.glyph, "●");
        assert!(!t.label.is_empty());
    }

    #[test]
    fn hermes_theme_is_neutral() {
        let t = harness_theme(&Harness::Hermes);
        assert_eq!(t.color, "#8a9099");
        assert_eq!(t.glyph, "●");
        assert!(!t.label.is_empty());
    }
}
