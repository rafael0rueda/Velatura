//! The app's own text, and the plain-language summary of a plan.
//!
//! Every string a person reads goes through [`tr`], so it can be translated.
//! Source text is US English and follows the GNOME writing guidelines.

use velatura_engine::plan::{Change, Plan};

/// Look a string up in the translation catalog.
pub fn tr(text: &str) -> String {
    velatura_text::tr(text)
}

/// Pick the form for `n` and put the number in place of `{n}`.
pub fn plural(one: &str, many: &str, n: usize) -> String {
    velatura_text::plural(one, many, n)
}

/// What a plan does, counted by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub created: usize,
    pub updated: usize,
    pub lines_added: usize,
    pub settings: usize,
    pub removed: usize,
}

pub fn count(plan: &Plan) -> Counts {
    let mut c = Counts::default();
    for change in plan.changes() {
        match change {
            Change::CreateDir { .. } | Change::RemoveDir { .. } => {}
            Change::Owned { content, pre, .. } => match (content, pre) {
                (Some(_), None) => c.created += 1,
                (Some(_), Some(_)) => c.updated += 1,
                (None, _) => c.removed += 1,
            },
            Change::Block {
                content,
                body_before,
                body_after,
                ..
            } => match (content, body_before, body_after) {
                (None, _, _) => c.removed += 1,
                (_, None, Some(_)) => c.lines_added += 1,
                (_, Some(_), None) => c.removed += 1,
                _ => c.updated += 1,
            },
            Change::Setting { .. } => c.settings += 1,
        }
    }
    c
}

/// The body of the confirmation: what will happen, in whole sentences.
pub fn summary(c: Counts) -> String {
    let mut parts = Vec::new();
    if c.lines_added > 0 {
        parts.push(plural(
            "Adds one line to a settings file.",
            "Adds one line to each of {n} settings files.",
            c.lines_added,
        ));
    }
    if c.created > 0 {
        parts.push(plural("Creates {n} file.", "Creates {n} files.", c.created));
    }
    if c.updated > 0 {
        parts.push(plural(
            "Updates {n} file made by Velatura.",
            "Updates {n} files made by Velatura.",
            c.updated,
        ));
    }
    if c.settings > 0 {
        parts.push(plural(
            "Changes {n} system setting.",
            "Changes {n} system settings.",
            c.settings,
        ));
    }
    if c.removed == 0 {
        parts.push(tr("Nothing is deleted."));
    }
    parts.push(tr("This can be undone at any time."));
    parts.join(" ")
}

/// Turn "Left out: reason" from the shared messages into "Reason".
pub fn reason(text: &str) -> String {
    let rest = text
        .strip_prefix(velatura_text::left_out_prefix().as_str())
        .unwrap_or(text);
    let mut chars = rest.chars();
    chars
        .next()
        .map(|f| f.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reads_as_sentences() {
        let first = Counts {
            created: 4,
            lines_added: 1,
            settings: 3,
            ..Counts::default()
        };
        assert_eq!(
            summary(first),
            "Adds one line to a settings file. Creates 4 files. Changes 3 system settings. Nothing is deleted. This can be undone at any time."
        );
        let again = Counts {
            updated: 1,
            settings: 1,
            ..Counts::default()
        };
        assert_eq!(
            summary(again),
            "Updates 1 file made by Velatura. Changes 1 system setting. Nothing is deleted. This can be undone at any time."
        );
        let removing = Counts {
            removed: 2,
            ..Counts::default()
        };
        assert_eq!(summary(removing), "This can be undone at any time.");
    }

    #[test]
    fn reasons_lose_their_prefix() {
        assert_eq!(
            reason("Left out: High Contrast is on"),
            "High Contrast is on"
        );
        assert_eq!(reason("Not installed"), "Not installed");
    }
}
