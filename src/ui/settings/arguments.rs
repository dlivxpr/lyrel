//! Settings argument-field syntax shared by shell editing and saved agent commands.
//! Parsing and formatting are pure: callers own validation display and persistence.

/// Reads the Settings "Shell Arguments" field as a command line (#551).
///
/// The field is one line of text standing in for the `shell.args` array, so
/// this and [`join_shell_args`] have to be exact inverses of each other — the
/// field is refilled from the array every time Settings opens, and committed
/// back on every blur. Quoting rules are the smallest set that can spell any
/// argv: single quotes take everything literally, double quotes take everything
/// literally except `\"` and `\\`, and whitespace outside quotes separates
/// words.
///
/// A lone backslash outside quotes is *not* an escape, unlike a POSIX shell:
/// these arguments are handed to the process as argv without a shell in
/// between, and `--dir C:\Users\me` has to keep its backslashes on the platform
/// where that is how a path is spelled. `#` starts no comment here either, for
/// the same reason: nothing in an argv list is a comment, and swallowing the
/// rest of the line would be the silent rewrite this whole change exists to
/// stop.
///
/// Returns `Err` when a quote never closes: that text names no argv at all.
pub(crate) fn split_shell_args(input: &str) -> Result<Vec<String>, ()> {
    let mut words = Vec::new();
    let mut current = String::new();
    // A word can be empty and still be a word — `''` is a real argument that
    // whitespace splitting cannot spell, so emptiness alone cannot end one.
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            // Only inside double quotes, and only for the two characters that
            // would otherwise be unspellable there.
            (Some('"'), '\\') => match chars.clone().next() {
                Some(next @ ('"' | '\\')) => {
                    chars.next();
                    current.push(next);
                }
                _ => current.push('\\'),
            },
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

/// Writes an argv array back into the Settings "Shell Arguments" field (#551).
///
/// The exact inverse of [`split_shell_args`], and quotes only what it must:
/// `join(" ")` cannot spell an argument that contains a space, so a legal
/// `"args": ["-c", "echo hi"]` used to refill as three words and re-commit as
/// three argv on the next blur. Arguments that need no quoting are written
/// bare, so the common `--login --color=auto` still reads as the user typed it.
pub(crate) fn join_shell_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_shell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_shell_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
    {
        return arg.to_string();
    }
    // Single quotes need no escapes at all, so they are the first choice; an
    // argument containing one falls back to double quotes, which can escape.
    if !arg.contains('\'') {
        return format!("'{arg}'");
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{join_shell_args, split_shell_args};

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    /// The invariant #551 is fixed with: what `build_shell_inputs` writes into
    /// the Arguments field must parse back to exactly the argv it came from, so
    /// that opening Settings and clicking away cannot rewrite `config.json`.
    #[test]
    fn shell_arguments_round_trip_between_field_text_and_argv() {
        for case in [
            &["-l"][..],
            &["-c", "echo hi"],
            // A space-join cannot even spell these three.
            &[""],
            &["don't"],
            &["--say", "it's \"quoted\""],
            // Backslashes are how a path is spelled on Windows, and the field
            // has to be able to carry one either bare or inside quotes.
            &[r"--dir", r"C:\Users\me"],
            &[r"--dir", r"C:\Program Files\lyrel"],
            &["--tag", "#1"],
        ] {
            let argv = argv(case);
            let field_text = join_shell_args(&argv);
            assert_eq!(
                split_shell_args(&field_text).expect("lyrel's own refill must parse"),
                argv,
                "round trip through {field_text:?}"
            );
        }
        // Nothing that does not need quoting acquires any: the everyday field
        // still reads exactly like the user typed it.
        assert_eq!(
            join_shell_args(&argv(&["-l", "--color=auto", r"C:\Users\me", "#1"])),
            r"-l --color=auto C:\Users\me #1"
        );
        assert_eq!(join_shell_args(&[]), "");
    }

    /// What the user types has to reach argv meaning what it says: quoted text
    /// is one argument, and nothing outside quotes is an escape or a comment.
    #[test]
    fn shell_arguments_split_the_way_the_field_promises() {
        assert_eq!(
            split_shell_args("--login -c \"echo hi\"").expect("balanced quotes parse"),
            argv(&["--login", "-c", "echo hi"])
        );
        assert_eq!(
            split_shell_args("-c 'echo  hi'").expect("single quotes parse"),
            argv(&["-c", "echo  hi"])
        );
        // A backslash outside quotes is a character, not an escape: there is no
        // shell between this field and argv, and POSIX rules would quietly turn
        // a Windows path into `C:Usersme`.
        assert_eq!(
            split_shell_args(r"--dir C:\Users\me").expect("a bare path parses"),
            argv(&["--dir", r"C:\Users\me"])
        );
        // `#` starts no comment either — swallowing the rest of the line is the
        // silent rewrite this change exists to stop.
        assert_eq!(
            split_shell_args("--tag #1 --verbose").expect("a hash parses"),
            argv(&["--tag", "#1", "--verbose"])
        );
        // Inside double quotes, and only there, `\"` and `\\` stand for
        // themselves — that is how `join_shell_args` spells an argument that
        // carries both kinds of quote.
        assert_eq!(
            split_shell_args("\"it's \\\"quoted\\\"\"").expect("escapes parse"),
            argv(&["it's \"quoted\""])
        );
        assert_eq!(
            split_shell_args("'' -l").expect("an empty argument parses"),
            argv(&["", "-l"])
        );
    }

    /// The other half of the #551 contract: a value that cannot become argv is
    /// refused, not shredded into fragments that happen to contain quotes.
    #[test]
    fn unbalanced_quotes_are_refused_not_shredded() {
        assert!(split_shell_args("-c \"echo hi").is_err());
        assert!(split_shell_args("--name 'lyrel").is_err());
        assert!(split_shell_args("\"a\\\"").is_err());
    }
}
