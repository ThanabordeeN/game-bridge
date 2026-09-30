//! Prompt construction and output cleanup for translation (§5).
//!
//! SPEC §5 asks for something more specific than "translate this": the model
//! must preserve names, map locations, and player names; understand gaming
//! slang; shorten where appropriate; and avoid unnecessary explanation.
//!
//! Each of those is a line in the system prompt below. They are rules rather
//! than wishes because a general-purpose translation prompt produces output
//! that is *wrong for this product* in ways that look like bugs:
//!
//! * A literal rendering of a callout is too long to read mid-fight.
//! * A translated map name sends teammates to the wrong place.
//! * An explanation ("This appears to be Thai slang meaning…") is useless in a
//!   one-line subtitle.
//!
//! # Why the output needs cleaning
//!
//! Even with an explicit "output only the translation" instruction, models
//! reliably add things: quotation marks, a `Translation:` label, a trailing
//! note. [`sanitize_model_output`] removes the patterns that appear in practice,
//! and the tests here pin down which ones.

use game_bridge_protocol::Language;

/// Build the system prompt for a translation.
///
/// `context` is the optional game hint from SPEC §5, e.g. `"competitive_fps"`.
pub fn build_system_prompt(
    source: Language,
    target: Language,
    context: Option<&str>,
) -> String {
    let source_name = source.display_name();
    let target_name = target.display_name();

    let mut prompt = String::with_capacity(900);

    prompt.push_str(
        "You translate spoken voice-chat callouts between players in a live video game.\n\n",
    );
    prompt.push_str(&format!(
        "Translate the user's message from {source_name} into {target_name}.\n\n"
    ));

    prompt.push_str("Rules:\n");
    prompt.push_str(
        "1. Output only the translation. No preamble, no label, no quotation marks, no \
         explanation, no notes.\n",
    );
    prompt.push_str(
        "2. Preserve proper nouns exactly as written: player names, map names, callout names, \
         weapon names, and game titles. Do not translate or transliterate them.\n",
    );
    prompt.push_str(&format!(
        "3. Translate gaming slang into the natural equivalent callout in {target_name}, not \
         word for word.\n"
    ));
    prompt.push_str(
        "4. Keep it short. This is read or heard during a match: a brief rendering that conveys \
         the call is better than a longer, more literal one.\n",
    );
    prompt.push_str(&format!(
        "5. If the message is already in {target_name}, return it unchanged.\n"
    ));
    prompt.push_str(
        "6. If the message is unintelligible, empty, or contains nothing translatable, return \
         nothing at all.\n",
    );

    if let Some(context) = context.map(str::trim).filter(|c| !c.is_empty()) {
        prompt.push_str(&format!(
            "\nThe game context is: {context}. Prefer terminology and phrasing that fit it.\n"
        ));
    }

    prompt
}

/// Labels a model may prefix its answer with, which are not part of the answer.
const LABEL_PREFIXES: &[&str] = &[
    "translation:",
    "translated:",
    "translation -",
    "english:",
    "thai:",
    "output:",
    "answer:",
    "translation into english:",
    "translation into thai:",
    "คำแปล:",
    "แปล:",
    "翻译:",
    "翻訳:",
    "traducción:",
    "traduction:",
];

/// Clean a model's raw output for display as a subtitle.
///
/// Removes, in order:
///
/// 1. a leading label such as `Translation:`,
/// 2. matching quotation marks around the whole answer, including the Unicode
///    forms models prefer,
/// 3. line breaks and runs of whitespace, which a one-line subtitle cannot use,
/// 4. a result that is only punctuation, which means the model had nothing to
///    say.
///
/// It deliberately does **not** strip trailing parentheses. `"Push B (two of
/// them)"` is a legitimate callout, and a heuristic that removed it would
/// silently delete content mid-match.
pub fn sanitize_model_output(raw: &str) -> String {
    let mut text = raw.trim().to_string();

    // 1. A leading label, case-insensitively.
    let lowered = text.to_lowercase();
    for label in LABEL_PREFIXES {
        if lowered.starts_with(label) {
            // Cut using the original string's byte length, not the lowercased
            // copy's: lowercasing can change a character's byte length, and
            // slicing by the wrong offset panics on multibyte input such as
            // Thai.
            text = text[label.len()..].trim_start().to_string();
            break;
        }
    }

    // 2. Surrounding quotation marks, repeated in case of nesting.
    loop {
        let trimmed = text.trim();
        let stripped = strip_surrounding_quotes(trimmed);
        if stripped.len() == trimmed.len() {
            break;
        }
        text = stripped.trim().to_string();
    }

    // 3. Collapse newlines and whitespace runs into single spaces. A subtitle
    //    is one line, and a model that emits a bulleted list would otherwise
    //    render as overlapping text.
    let collapsed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // 4. Nothing but punctuation means there was nothing to say.
    if collapsed.is_empty() || collapsed.chars().all(|c| c.is_ascii_punctuation()) {
        return String::new();
    }

    collapsed
}

/// Remove one matching pair of surrounding quotation marks.
fn strip_surrounding_quotes(text: &str) -> &str {
    const PAIRS: [(char, char); 5] = [
        ('"', '"'),
        ('\'', '\''),
        ('\u{201C}', '\u{201D}'), // “ ”
        ('\u{2018}', '\u{2019}'), // ‘ ’
        ('\u{300C}', '\u{300D}'), // 「 」
    ];

    for (open, close) in PAIRS {
        if text.starts_with(open) && text.ends_with(close) {
            let inner = &text[open.len_utf8()..];
            if inner.len() >= close.len_utf8() {
                return &inner[..inner.len() - close.len_utf8()];
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> String {
        build_system_prompt(Language::Thai, Language::English, None)
    }

    #[test]
    fn the_prompt_names_both_languages() {
        let prompt = prompt();
        assert!(prompt.contains("Thai"), "{prompt}");
        assert!(prompt.contains("English"), "{prompt}");
    }

    #[test]
    fn the_prompt_carries_every_rule_from_spec_section_5() {
        // §5's five requirements, each traceable to a line of the prompt.
        let prompt = prompt().to_lowercase();
        assert!(prompt.contains("preserve proper nouns"), "names preserved");
        assert!(prompt.contains("map names"), "map locations preserved");
        assert!(prompt.contains("player names"), "player names preserved");
        assert!(prompt.contains("gaming slang"), "slang understood");
        assert!(prompt.contains("keep it short"), "shortened when appropriate");
        assert!(
            prompt.contains("no explanation"),
            "explanations suppressed"
        );
    }

    #[test]
    fn the_prompt_demands_bare_output() {
        // Without this the model labels its answer and the overlay shows
        // "Translation: Enemy is behind us."
        let prompt = prompt().to_lowercase();
        assert!(prompt.contains("output only the translation"));
        assert!(prompt.contains("no preamble"));
        assert!(prompt.contains("no quotation marks"));
    }

    #[test]
    fn the_prompt_tells_the_model_to_shorten() {
        let prompt = prompt().to_lowercase();
        assert!(
            prompt.contains("brief rendering"),
            "the shortening rule must state the tradeoff, not just say 'be short'"
        );
    }

    #[test]
    fn a_context_hint_is_included_when_present() {
        let with = build_system_prompt(Language::Thai, Language::English, Some("competitive_fps"));
        assert!(with.contains("competitive_fps"));
        assert!(with.contains("game context"));
    }

    #[test]
    fn a_blank_context_hint_is_omitted() {
        // An empty string would produce "The game context is: ." which is worse
        // than saying nothing.
        for blank in ["", "   ", "\t\n"] {
            let prompt = build_system_prompt(Language::Thai, Language::English, Some(blank));
            assert!(!prompt.contains("game context"), "blank context leaked: {blank:?}");
        }
    }

    #[test]
    fn no_context_produces_no_context_line() {
        assert!(!prompt().contains("game context"));
    }

    #[test]
    fn the_prompt_is_direction_aware() {
        // Translating the other way must say so.
        let reversed = build_system_prompt(Language::English, Language::Thai, None);
        assert!(reversed.contains("from English into Thai"), "{reversed}");
    }

    #[test]
    fn the_prompt_is_stable_across_calls() {
        // A prompt that varies between calls makes latency and cost
        // unpredictable and defeats provider-side caching.
        assert_eq!(prompt(), prompt());
    }

    #[test]
    fn a_clean_translation_passes_through_unchanged() {
        assert_eq!(
            sanitize_model_output("Two are pushing B."),
            "Two are pushing B."
        );
    }

    #[test]
    fn surrounding_quotes_are_removed() {
        assert_eq!(sanitize_model_output("\"Enemy is behind us.\""), "Enemy is behind us.");
        assert_eq!(sanitize_model_output("'Enemy is behind us.'"), "Enemy is behind us.");
        // The curly forms models prefer.
        assert_eq!(
            sanitize_model_output("\u{201C}Enemy is behind us.\u{201D}"),
            "Enemy is behind us."
        );
        assert_eq!(
            sanitize_model_output("\u{300C}Enemy is behind us.\u{300D}"),
            "Enemy is behind us."
        );
    }

    #[test]
    fn nested_quotes_are_removed_repeatedly() {
        assert_eq!(
            sanitize_model_output("\"'Enemy is behind us.'\""),
            "Enemy is behind us."
        );
    }

    #[test]
    fn a_leading_label_is_removed() {
        assert_eq!(
            sanitize_model_output("Translation: Two are pushing B."),
            "Two are pushing B."
        );
        assert_eq!(
            sanitize_model_output("translation: Two are pushing B."),
            "Two are pushing B."
        );
        assert_eq!(
            sanitize_model_output("TRANSLATION: Two are pushing B."),
            "Two are pushing B."
        );
        assert_eq!(
            sanitize_model_output("คำแปล: มีสองคนดันบี"),
            "มีสองคนดันบี"
        );
    }

    #[test]
    fn a_label_followed_by_quotes_is_fully_cleaned() {
        // The combination that actually appears in practice.
        assert_eq!(
            sanitize_model_output("Translation: \"Two are pushing B.\""),
            "Two are pushing B."
        );
    }

    #[test]
    fn multibyte_labels_do_not_panic_on_a_byte_boundary() {
        // Lowercasing can change byte length; slicing the original by the
        // lowercased length would panic mid-character. This is the regression
        // test for that.
        let inputs = [
            "คำแปล: ทดสอบ",
            "แปล: ทดสอบระบบ",
            "翻译: 敌人在后面",
            "翻訳: 敵が後ろにいる",
            "traducción: enemigo detrás",
            "traduction: ennemi derrière",
        ];
        for input in inputs {
            let cleaned = sanitize_model_output(input);
            assert!(!cleaned.is_empty(), "input {input:?} produced nothing");
            assert!(
                !cleaned.contains(':') && !cleaned.contains('：'),
                "label survived in {input:?} -> {cleaned:?}"
            );
        }
    }

    #[test]
    fn newlines_collapse_into_a_single_line() {
        // A subtitle is one line; a model that answers with a list would
        // otherwise render as overlapping text.
        assert_eq!(
            sanitize_model_output("Two are pushing B.\nThey are behind the wall."),
            "Two are pushing B. They are behind the wall."
        );
        assert_eq!(
            sanitize_model_output("  Two   are\n\npushing  B.  "),
            "Two are pushing B."
        );
    }

    #[test]
    fn a_punctuation_only_answer_becomes_empty() {
        // Rule 6: the model had nothing translatable to say. Rendering "..." as
        // a subtitle is noise.
        assert_eq!(sanitize_model_output("..."), "");
        assert_eq!(sanitize_model_output("\"\""), "");
        assert_eq!(sanitize_model_output("   "), "");
        assert_eq!(sanitize_model_output("---"), "");
    }

    #[test]
    fn trailing_parentheses_are_preserved() {
        // Deliberately not stripped: this is legitimate callout content, and a
        // heuristic that removed it would delete information mid-match.
        assert_eq!(
            sanitize_model_output("Push B (two of them)"),
            "Push B (two of them)"
        );
    }

    #[test]
    fn thai_text_without_a_label_survives_intact() {
        let thai = "มีสองคนกำลังดันมาจาก B";
        assert_eq!(sanitize_model_output(thai), thai);
    }

    #[test]
    fn whitespace_inside_thai_is_collapsed_without_corrupting_it() {
        // Thai does not use spaces between words; collapsing must not insert or
        // remove characters beyond the whitespace itself.
        assert_eq!(
            sanitize_model_output("  มีสองคน  กำลังดัน  "),
            "มีสองคน กำลังดัน"
        );
    }

    #[test]
    fn an_empty_model_answer_stays_empty() {
        assert_eq!(sanitize_model_output(""), "");
    }

    #[test]
    fn a_quoted_empty_string_does_not_panic() {
        // `""` is two characters that both look like a quote; naive slicing
        // would produce a negative-length range.
        assert_eq!(sanitize_model_output("\"\""), "");
        assert_eq!(sanitize_model_output("''"), "");
        assert_eq!(sanitize_model_output("\u{201C}\u{201D}"), "");
    }

    #[test]
    fn a_single_quote_character_does_not_panic() {
        assert_eq!(sanitize_model_output("\""), "");
        assert_eq!(sanitize_model_output("'"), "");
    }

    #[test]
    fn sanitizing_is_idempotent() {
        // The UI may sanitize a line more than once as partials arrive.
        for raw in [
            "Translation: \"Two are pushing B.\"",
            "  Enemy   behind us.  ",
            "คำแปล: มีสองคน",
            "...",
        ] {
            let once = sanitize_model_output(raw);
            assert_eq!(sanitize_model_output(&once), once, "not idempotent for {raw:?}");
        }
    }
}