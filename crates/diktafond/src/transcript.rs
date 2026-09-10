//! Guards against decoder failures that arrive as text.

/// Longest run of one word accepted as speech. A greedy attention decoder
/// that misses its end-of-sequence token repeats a word until it finds one:
/// a 1.7s mid-phrase chunk came back as "and" 281 times in this user's
/// history, and transcribe.cpp exposes no repetition control for Canary.
/// Nobody dictates a word four times in a row, so a longer run is the
/// decoder looping and collapses to the one word it did hear. Two- and
/// three-word cycles are left alone: "the author and the album, author and
/// the album" was real dictation.
const MAX_SPOKEN_RUN: usize = 3;

/// `Some(text)` with every run longer than [`MAX_SPOKEN_RUN`] cut to one
/// word, or `None` when there was no such run and the text stands as is.
pub fn collapse_word_loops(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut kept = Vec::with_capacity(words.len());
    let mut looped = false;
    let mut i = 0;
    while i < words.len() {
        let run = words[i..]
            .iter()
            .take_while(|word| word.eq_ignore_ascii_case(words[i]))
            .count();
        if run > MAX_SPOKEN_RUN {
            looped = true;
            kept.push(words[i]);
        } else {
            kept.extend_from_slice(&words[i..i + run]);
        }
        i += run;
    }
    looped.then(|| kept.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_loop_collapses_to_one_word() {
        let looped = format!("maybe again on the {}", "and ".repeat(281).trim_end());
        assert_eq!(
            collapse_word_loops(&looped).as_deref(),
            Some("maybe again on the and")
        );
    }

    #[test]
    fn loop_is_matched_case_insensitively() {
        assert_eq!(
            collapse_word_loops("No no NO no please").as_deref(),
            Some("No please")
        );
    }

    #[test]
    fn spoken_repeats_and_cycles_are_kept() {
        assert_eq!(collapse_word_loops("no no no, I mean it"), None);
        assert_eq!(
            collapse_word_loops("between the author and the album author and the album"),
            None
        );
        assert_eq!(collapse_word_loops(""), None);
    }
}
