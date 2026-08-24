//! S1-mini's control line, the first line of every polish prompt. The model
//! was trained on exactly these three settings and these values (see the
//! superwhisper/s1-mini model card); free text in their place degrades the
//! polish or blanks it, so the settings window offers the vocabulary rather
//! than a text field.

pub struct Axis {
    /// As it appears in the line: `[Styling: formal]`.
    pub name: &'static str,
    pub values: &'static [&'static str],
    pub default: usize,
}

pub const STYLING: Axis = Axis {
    name: "Styling",
    values: &["casual", "semi-casual", "semi-formal", "formal"],
    default: 2,
};

pub const STRUCTURE: Axis = Axis {
    name: "Structure",
    values: &["prose", "lists"],
    default: 0,
};

pub const CONTEXT: Axis = Axis {
    name: "Context",
    values: &["general", "email"],
    default: 0,
};

/// In the order they appear in the line.
pub const AXES: [Axis; 3] = [STYLING, STRUCTURE, CONTEXT];

impl Axis {
    /// Which of this axis's values `line` selects. A line that omits the axis,
    /// or names something the model was never trained on, falls back to the
    /// default rather than being passed through.
    pub fn index_in(&self, line: &str) -> usize {
        let line = line.to_lowercase();
        let opening = format!("[{}:", self.name.to_lowercase());
        let Some(start) = line.find(&opening) else {
            return self.default;
        };
        let rest = &line[start + opening.len()..];
        let Some(end) = rest.find(']') else {
            return self.default;
        };
        let value = rest[..end].trim();
        self.values
            .iter()
            .position(|candidate| *candidate == value)
            .unwrap_or(self.default)
    }

    pub fn value(&self, index: usize) -> &'static str {
        self.values.get(index).unwrap_or(&self.values[self.default])
    }

    /// The value as the settings window shows it: "semi-formal" reads as
    /// "Semi-formal".
    pub fn label(&self, index: usize) -> String {
        let value = self.value(index);
        let mut chars = value.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }
}

pub fn compose(styling: usize, structure: usize, context: usize) -> String {
    format!(
        "[Styling: {}] [Structure: {}] [Context: {}]",
        STYLING.value(styling),
        STRUCTURE.value(structure),
        CONTEXT.value(context)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_line_round_trips() {
        let line = compose(STYLING.default, STRUCTURE.default, CONTEXT.default);
        assert_eq!(
            line,
            "[Styling: semi-formal] [Structure: prose] [Context: general]"
        );
        assert_eq!(STYLING.index_in(&line), STYLING.default);
        assert_eq!(STRUCTURE.index_in(&line), STRUCTURE.default);
        assert_eq!(CONTEXT.index_in(&line), CONTEXT.default);
    }

    #[test]
    fn every_value_round_trips() {
        for styling in 0..STYLING.values.len() {
            for structure in 0..STRUCTURE.values.len() {
                for context in 0..CONTEXT.values.len() {
                    let line = compose(styling, structure, context);
                    assert_eq!(STYLING.index_in(&line), styling, "{line}");
                    assert_eq!(STRUCTURE.index_in(&line), structure, "{line}");
                    assert_eq!(CONTEXT.index_in(&line), context, "{line}");
                }
            }
        }
    }

    #[test]
    fn untrained_and_malformed_lines_fall_back() {
        // A value the model never saw, free text, a missing axis, and an
        // unclosed one all land on the defaults instead of reaching the model.
        for line in [
            "[Styling: shakespearean] [Structure: prose] [Context: general]",
            "please make it sound nicer",
            "",
            "[Styling: formal",
            "[Structure: lists]",
        ] {
            assert_eq!(STYLING.index_in(line), STYLING.default, "{line}");
        }
        // A present axis still wins when its neighbours are missing.
        assert_eq!(STRUCTURE.index_in("[Structure: lists]"), 1);
    }

    #[test]
    fn parsing_ignores_case_and_spacing() {
        assert_eq!(STYLING.index_in("[styling:   FORMAL ]"), 3);
        assert_eq!(CONTEXT.index_in("[CONTEXT: Email]"), 1);
    }

    #[test]
    fn labels_are_capitalised_for_display() {
        assert_eq!(STYLING.label(1), "Semi-casual");
        assert_eq!(CONTEXT.label(1), "Email");
    }
}
